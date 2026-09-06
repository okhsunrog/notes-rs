package dev.okhsunrog.mobile_system

import org.junit.Assert.*
import org.junit.Test

class DisplayKindTest {
    @Test fun onyxDevicesReportAnEinkPanelRegardlessOfManufacturerCasing() {
        assertEquals(DisplayKind.EINK, DisplayKind.of("ONYX"))
        assertEquals(DisplayKind.EINK, DisplayKind.of("onyx"))
        assertEquals(DisplayKind.EINK, DisplayKind.of(" Onyx "))
    }

    @Test fun everyOtherManufacturerReportsAnOrdinaryPanel() {
        assertEquals(DisplayKind.LCD, DisplayKind.of("Google"))
        assertEquals(DisplayKind.LCD, DisplayKind.of("samsung"))
        assertEquals(DisplayKind.LCD, DisplayKind.of(""))
        assertEquals(DisplayKind.LCD, DisplayKind.of("onyxx"))
    }

    @Test fun colorPanelStaysUnknownWithoutADocumentedSdkQuery() {
        assertNull(DisplayKind.colorPanel())
    }
}
