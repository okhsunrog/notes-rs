package dev.okhsunrog.mobile_system

/**
 * Panel technology of the device the app runs on. E-ink is decided by manufacturer: the platform
 * exposes no display-technology query, and the WebView does not report `(update: slow)` either.
 */
internal object DisplayKind {
    const val EINK = "eink"
    const val LCD = "lcd"

    private val EINK_MANUFACTURERS = setOf("onyx")

    fun of(manufacturer: String): String =
        if (manufacturer.trim().lowercase() in EINK_MANUFACTURERS) EINK else LCD

    /**
     * Whether the panel renders color. The ONYX SDK (onyxsdk-device, onyxsdk-base) publishes no
     * color-panel query — only Regal support and CTM brightness/color-filter setters — and hidden
     * APIs are out of bounds, so this stays unknown and the frontend decides.
     */
    fun colorPanel(): Boolean? = null
}
