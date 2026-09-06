package dev.okhsunrog.mobile_system

import com.onyx.android.sdk.api.device.epd.UpdateMode
import dev.okhsunrog.mobile_system.DisplayModeStack.Layer
import org.junit.Assert.*
import org.junit.Test

class DisplayModeStackTest {
    private val calls = mutableListOf<String>()
    private val stack = DisplayModeStack(
        apply = { calls.add(it.name) },
        restoreRaw = { calls.add("restore") },
    )

    @Test fun theTopmostSetLayerDecidesWhatThePanelUses() {
        stack.set(Layer.BASE, UpdateMode.REGAL)
        stack.set(Layer.SESSION, UpdateMode.GU)
        stack.set(Layer.TRANSIENT, UpdateMode.HAND_WRITING_REPAINT_MODE)
        assertEquals(UpdateMode.HAND_WRITING_REPAINT_MODE, stack.effective)
        assertEquals(listOf("REGAL", "GU", "HAND_WRITING_REPAINT_MODE"), calls)
    }

    @Test fun aWeakerLayerSetUnderAStrongerOneNeverReachesThePanel() {
        stack.set(Layer.SESSION, UpdateMode.GU)
        calls.clear()
        // The display profile arriving while the ink editor is open must not slow writing down.
        stack.set(Layer.BASE, UpdateMode.REGAL)
        assertEquals(UpdateMode.GU, stack.effective)
        assertEquals(listOf("GU"), calls)
    }

    @Test fun clearingALayerReAppliesTheOneBelowItInsteadOfRestoring() {
        stack.set(Layer.BASE, UpdateMode.REGAL)
        stack.set(Layer.SESSION, UpdateMode.GU)
        calls.clear()
        stack.clear(Layer.SESSION)
        assertEquals(UpdateMode.REGAL, stack.effective)
        assertTrue(stack.owned)
        assertEquals(listOf("REGAL"), calls)
    }

    @Test fun theRawModeIsRestoredOnlyWhenTheLastLayerClears() {
        stack.set(Layer.BASE, UpdateMode.REGAL)
        stack.set(Layer.SESSION, UpdateMode.GU)
        stack.set(Layer.TRANSIENT, UpdateMode.HAND_WRITING_REPAINT_MODE)
        stack.clear(Layer.TRANSIENT)
        stack.clear(Layer.SESSION)
        assertFalse(calls.contains("restore"))
        stack.clear(Layer.BASE)
        assertEquals("restore", calls.last())
        assertNull(stack.effective)
        assertFalse(stack.owned)
    }

    @Test fun theOrderLayersClearInDoesNotChangeWhatIsRestored() {
        stack.set(Layer.BASE, UpdateMode.REGAL)
        stack.set(Layer.SESSION, UpdateMode.GU)
        calls.clear()
        // The profile can be switched back to standard while the editor is still open.
        stack.clear(Layer.BASE)
        assertEquals(UpdateMode.GU, stack.effective)
        stack.clear(Layer.SESSION)
        assertEquals(listOf("GU", "restore"), calls)
        assertNull(stack.effective)
    }

    @Test fun repeatingASetIsIgnoredButChangingItsModeIsNot() {
        stack.set(Layer.SESSION, UpdateMode.GU)
        stack.set(Layer.SESSION, UpdateMode.GU)
        assertEquals(listOf("GU"), calls)
        stack.set(Layer.SESSION, UpdateMode.REGAL)
        assertEquals(listOf("GU", "REGAL"), calls)
    }

    @Test fun clearingALayerThatWasNeverSetDoesNothing() {
        stack.clear(Layer.SESSION)
        stack.set(Layer.BASE, UpdateMode.REGAL)
        stack.clear(Layer.TRANSIENT)
        // A stray release must not hand the view back while the profile still owns it.
        assertEquals(listOf("REGAL"), calls)
        assertTrue(stack.owned)
    }

    @Test fun aGesturesLayerComesAndGoesWithoutDisturbingTheSessionBelow() {
        stack.set(Layer.SESSION, UpdateMode.GU)
        calls.clear()
        repeat(3) {
            stack.set(Layer.TRANSIENT, UpdateMode.HAND_WRITING_REPAINT_MODE)
            stack.clear(Layer.TRANSIENT)
        }
        assertEquals(UpdateMode.GU, stack.effective)
        assertEquals(
            List(3) { listOf("HAND_WRITING_REPAINT_MODE", "GU") }.flatten(),
            calls,
        )
    }
}
