package dev.okhsunrog.mobile_system

import android.os.Build
import org.lsposed.hiddenapibypass.HiddenApiBypass

/**
 * Vendor firmware APIs (android.onyx.*, the View update-mode accessors) are hidden from apps
 * targeting recent Android versions. Every entry point that touches them — the ink session, the
 * display-mode stack, the profile command — must lift the restriction first, and the first of
 * them to run is not always the ink editor: the display profile is applied at startup.
 */
internal object VendorAccess {
    @Volatile
    private var granted: Boolean? = null

    /** True when the hidden APIs are reachable; false when the bypass was refused. */
    fun ensure(): Boolean {
        granted?.let { return it }
        val result = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            HiddenApiBypass.addHiddenApiExemptions("Landroid/onyx/", "Landroid/view/View;")
        } else {
            true
        }
        granted = result
        return result
    }
}
