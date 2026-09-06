package dev.okhsunrog.mobile_system

/** The two firmware switches, plus the geometry that has to be re-pushed before they go back on. */
internal interface RawDrawingSwitches {
    fun render(enabled: Boolean)
    fun input(enabled: Boolean)

    /** `setLimitRect`/`setExcludeRect`: the SDK caches the view position, so it is pushed again. */
    fun pushRects()

    /**
     * `resetPenDefaultRawDrawing`: brush ink on, hardware eraser ink off. `setRawDrawingEnabled`
     * used to do this as a side effect, and the firmware keeps these flags per device, not per
     * session, so a resume that skipped it could inherit a brush another app switched off.
     */
    fun resetDefaults()
}

/**
 * Render and input are separate switches and the order matters.
 *
 * Pausing turns rendering off first, so no stroke is painted from input that is already on its way
 * out. Resuming re-pushes the drawable region, then enables input before rendering — the order
 * `ResumeRawDrawingRequest` uses in stock Notes; the other way round the firmware paints ghost ink
 * from the events that arrive before the reader is armed.
 *
 * `setRawDrawingEnabled` is deliberately not used: it flips both at once, which is exactly what
 * makes a paused pen indistinguishable from a closed session.
 */
internal class RawDrawingGate(private val switches: RawDrawingSwitches) {
    fun pause() {
        switches.render(false)
        switches.input(false)
    }

    /** @param render whether the active tool wants firmware ink, not just raw points. */
    fun resume(render: Boolean) {
        switches.pushRects()
        switches.resetDefaults()
        switches.input(true)
        switches.render(render)
    }
}
