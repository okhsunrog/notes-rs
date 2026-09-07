package dev.okhsunrog.mobile_system

import android.app.Activity
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.hardware.input.InputManager
import android.view.InputDevice
import android.view.MotionEvent
import android.util.Log
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import com.onyx.android.sdk.api.device.epd.EpdController
import com.onyx.android.sdk.api.device.epd.UpdateMode
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class SystemBarsStyleArgs {
    var darkBackground: Boolean = false
}

@InvokeArg
class DisplayProfileArgs {
    var eink: Boolean = false
}

@InvokeArg
class InkSuppressArgs {
    var reason: String = ""
    var active: Boolean = false
}

/** No enum value means "the profile claims no base mode"; UpdateMode.None is a real mode. */
private const val NO_BASE_MODE = "none"

@TauriPlugin
class MobileSystemPlugin(private val activity: Activity) : Plugin(activity), InputManager.InputDeviceListener {
    private val inputManager = activity.getSystemService(InputManager::class.java)
    private var inkWebView: WebView? = null
    private var onyxInk: OnyxInk? = null
    /**
     * The view the ink session was built on. Weak: a replaced WebView must stay
     * collectable, and the reference is only ever used for an identity check.
     */
    private var onyxInkWebView: java.lang.ref.WeakReference<WebView>? = null
    /**
     * The view's update-mode layers. It outlives every ink session — the display profile is set
     * before one exists and stays after it closes — so the plugin owns it and lends it out.
     */
    private var displayMode: ViewDisplayMode? = null
    private var displayModeWebView: java.lang.ref.WeakReference<WebView>? = null
    /**
     * Why the firmware pen is held down. Owned here rather than by the session: the page pauses
     * for a dialog or a focused field before the editor is mounted and after it is gone, and one
     * registry is what keeps `setRawDrawingEnabled` out of every call site.
     */
    private val inkPauses = InkPauseRegistry()

    override fun load(webView: WebView) {
        inkWebView = webView
        inputManager.registerInputDeviceListener(this, Handler(Looper.getMainLooper()))
        // One insets listener per view: the keyboard pauses the pen and switches the panel to
        // a text-friendly update mode, whether or not an ink session exists at the time.
        ViewCompat.setOnApplyWindowInsetsListener(webView) { _, insets ->
            imeChanged(webView, insets.getInsets(WindowInsetsCompat.Type.ime()).bottom > 0)
            insets
        }
        ViewCompat.requestApplyInsets(webView)
    }

    private var imeVisible = false

    private fun imeChanged(webView: WebView, visible: Boolean) {
        if (visible == imeVisible) return
        imeVisible = visible
        Log.d("OnyxInk", "ime visible=$visible")
        if (OnyxInk.supported()) {
            runCatching {
                val mode = displayMode(webView)
                if (visible) mode.set(DisplayModeStack.Layer.TEXT, UpdateMode.DU)
                else mode.clear(DisplayModeStack.Layer.TEXT)
            }.onFailure { Log.w("OnyxInk", "text display mode: ${it.message}") }
        }
        onyxInk?.imeChanged(visible)
    }

    @Suppress("OVERRIDE_DEPRECATION") // This plugin does not depend on AppCompat types.
    override fun onDestroy() {
        onyxInk?.destroy()
        onyxInk = null
        onyxInkWebView = null
        displayMode = null
        displayModeWebView = null
        inputManager.unregisterInputDeviceListener(this)
    }

    @Suppress("OVERRIDE_DEPRECATION")
    override fun onResume() {
        onyxInk?.onResume()
        inputDevicesChanged()
    }

    @Suppress("OVERRIDE_DEPRECATION")
    override fun onPause() {
        onyxInk?.onPause()
    }

    @Command
    fun configureOnyxInk(invoke: Invoke) {
        val args = invoke.parseArgs(OnyxInkArgs::class.java)
        activity.runOnUiThread {
            if (!OnyxInk.supported()) {
                invoke.resolve(JSObject().put("available", false).put("active", false))
                return@runOnUiThread
            }
            try {
                val current = inkWebView
                // wry re-creates the WebView on some configuration changes. The
                // old session still holds listeners, a palm region and a display
                // mode claim on a view that is never drawn again.
                if (onyxInk != null && current != null && onyxInkWebView?.get() !== current) {
                    onyxInk?.destroy()
                    onyxInk = null
                    onyxInkWebView = null
                }
                if (onyxInk == null && args.enabled) {
                    val webView = checkNotNull(current)
                    onyxInk = OnyxInk(activity, webView, displayMode(webView), inkPauses) {
                        trigger("onyxInk", it)
                    }
                    onyxInkWebView = java.lang.ref.WeakReference(webView)
                }
                invoke.resolve(onyxInk?.configure(args) ?: JSObject().put("available", true).put("active", false))
            } catch (error: Throwable) {
                invoke.reject("Could not start BOOX ink: ${error.message}")
            }
        }
    }

    /**
     * The page reports what is covering or competing with the sheet: a focused text field, an open
     * dialog, the soft keyboard the WebView itself put up. Each is a named reason; the pen comes
     * back only once every one of them is gone.
     */
    @Command
    fun suppressOnyxInk(invoke: Invoke) {
        val args = invoke.parseArgs(InkSuppressArgs::class.java)
        activity.runOnUiThread {
            try {
                require(args.reason.length in 1..128) { "a pause reason is required" }
                val changed =
                    if (args.active) inkPauses.pause(args.reason) else inkPauses.resume(args.reason)
                Log.d("OnyxInk", "suppress ${args.reason}=${args.active} changed=$changed paused=${inkPauses.reasons()}")
                if (changed) onyxInk?.pauseStateChanged()
                invoke.resolve(JSObject().put("paused", org.json.JSONArray(inkPauses.reasons())))
            } catch (error: Throwable) {
                invoke.reject("Could not suspend BOOX ink: ${error.message}")
            }
        }
    }

    @Command
    fun commitOnyxFrame(invoke: Invoke) {
        val args = invoke.parseArgs(OnyxFrameArgs::class.java)
        activity.runOnUiThread {
            // Without this a firmware failure here leaves the caller waiting on
            // a promise that is never settled.
            try {
                onyxInk?.commit(args)
                invoke.resolve()
            } catch (error: Throwable) {
                invoke.reject("Could not present the BOOX ink frame: ${error.message}")
            }
        }
    }

    /**
     * The layer stack for the WebView currently on screen. wry re-creates the view on some
     * configuration changes; the modes claimed on the old one go away with it, and the page that
     * reloads into the new view asks for its profile again.
     */
    private fun displayMode(webView: WebView): ViewDisplayMode {
        // The profile command can run before any ink session; the vendor calls it makes need
        // the same hidden-API access the ink editor arranges for itself.
        check(VendorAccess.ensure()) { "BOOX firmware display APIs are unavailable" }
        if (displayModeWebView?.get() !== webView) {
            displayMode = ViewDisplayMode(webView)
            displayModeWebView = java.lang.ref.WeakReference(webView)
        }
        return checkNotNull(displayMode)
    }

    @Command
    fun setDisplayProfile(invoke: Invoke) {
        val args = invoke.parseArgs(DisplayProfileArgs::class.java)
        activity.runOnUiThread {
            val webView = inkWebView
            if (!OnyxInk.supported() || webView == null) {
                // Named as a literal: a device without the panel never loads the vendor enum.
                invoke.resolve(JSObject()
                    .put("requested", if (args.eink) "REGAL" else NO_BASE_MODE)
                    .put("accepted", false))
                return@runOnUiThread
            }
            try {
                invoke.resolve(applyDisplayProfile(displayMode(webView), args.eink))
            } catch (error: Throwable) {
                invoke.reject("Could not set the panel refresh profile: ${error.message}")
            }
        }
    }

    /**
     * REGAL is the vendor's ghost-suppressing mode and not every panel or firmware honours it, so
     * the request is verified by reading the view back and downgraded to plain GU when it did not
     * stick. A stronger layer (the ink editor) owns the readback while it is open, so the profile
     * is left as asked for and the next call verifies it.
     */
    private fun applyDisplayProfile(view: ViewDisplayMode, eink: Boolean): JSObject {
        val result = JSObject()
        if (!eink) {
            view.clear(DisplayModeStack.Layer.BASE)
            return result.put("requested", NO_BASE_MODE)
                .put("accepted", true)
                .put("effectiveMode", view.readMode()?.name)
        }
        view.set(DisplayModeStack.Layer.BASE, UpdateMode.REGAL)
        val verifiable = view.effective == UpdateMode.REGAL
        val accepted = !verifiable || view.readMode() == UpdateMode.REGAL
        if (!accepted) view.set(DisplayModeStack.Layer.BASE, UpdateMode.GU)
        return result.put("requested", UpdateMode.REGAL.name)
            .put("accepted", accepted)
            .put("effectiveMode", view.readMode()?.name)
    }

    @Command
    fun requestFullRefresh(invoke: Invoke) {
        activity.runOnUiThread {
            val webView = inkWebView
            if (!OnyxInk.supported() || webView == null) {
                invoke.resolve()
                return@runOnUiThread
            }
            try {
                Log.d("OnyxInk", "full refresh requested")
                // A full-panel flash clears the ghosting the partial modes leave behind.
                EpdController.invalidate(webView, UpdateMode.GC)
                invoke.resolve()
            } catch (error: Throwable) {
                invoke.reject("Could not refresh the panel: ${error.message}")
            }
        }
    }

    override fun onInputDeviceAdded(deviceId: Int) = inputDevicesChanged()
    override fun onInputDeviceRemoved(deviceId: Int) = inputDevicesChanged()
    override fun onInputDeviceChanged(deviceId: Int) = inputDevicesChanged()

    private fun inputDevicesChanged() {
        trigger("inputDevicesChanged", JSObject())
    }

    @Command
    fun getStylusCapabilities(invoke: Invoke) {
        val devices = inputManager.inputDeviceIds.asSequence().mapNotNull { inputManager.getInputDevice(it) }
            .filter { !it.isVirtual && it.supportsSource(InputDevice.SOURCE_STYLUS) }
            .toList()
        val result = JSObject()
        result.put("available", devices.isNotEmpty())
        result.put("pressure", devices.any { device ->
            device.motionRanges.any { it.axis == MotionEvent.AXIS_PRESSURE && it.range > 0 }
        })
        result.put("tilt", devices.any { device ->
            device.motionRanges.any { it.axis == MotionEvent.AXIS_TILT && it.range > 0 }
        })
        invoke.resolve(result)
    }

    @Command
    fun getSafeAreaInsets(invoke: Invoke) {
        activity.runOnUiThread {
            val decorView = activity.window.decorView
            // A detached decor view never runs its posted work, so the caller
            // would wait forever instead of laying out with no insets.
            if (!decorView.isAttachedToWindow) {
                val zero = JSObject()
                for (edge in listOf("top", "right", "bottom", "left")) zero.put(edge, 0.0)
                invoke.resolve(zero)
                return@runOnUiThread
            }
            ViewCompat.requestApplyInsets(decorView)
            decorView.post {
                val density = activity.resources.displayMetrics.density
                val rootInsets = ViewCompat.getRootWindowInsets(decorView)
                val statusBars = rootInsets?.getInsets(WindowInsetsCompat.Type.statusBars())
                val navigationBars =
                    rootInsets?.getInsets(WindowInsetsCompat.Type.navigationBars())
                val cutout = rootInsets?.getInsets(WindowInsetsCompat.Type.displayCutout())

                val result = JSObject()
                result.put(
                    "top",
                    (maxOf(statusBars?.top ?: 0, cutout?.top ?: 0) / density).toDouble(),
                )
                result.put(
                    "right",
                    (maxOf(navigationBars?.right ?: 0, cutout?.right ?: 0) / density).toDouble(),
                )
                result.put(
                    "bottom",
                    (maxOf(navigationBars?.bottom ?: 0, cutout?.bottom ?: 0) / density).toDouble(),
                )
                result.put(
                    "left",
                    (maxOf(navigationBars?.left ?: 0, cutout?.left ?: 0) / density).toDouble(),
                )
                invoke.resolve(result)
            }
        }
    }

    @Command
    fun setSystemBarsStyle(invoke: Invoke) {
        val args = invoke.parseArgs(SystemBarsStyleArgs::class.java)
        activity.runOnUiThread {
            val controller =
                WindowCompat.getInsetsController(activity.window, activity.window.decorView)
            controller.isAppearanceLightStatusBars = !args.darkBackground
            controller.isAppearanceLightNavigationBars = !args.darkBackground
            invoke.resolve()
        }
    }

    @Command
    fun getDisplayInfo(invoke: Invoke) {
        val result = JSObject()
        result.put("kind", DisplayKind.of(Build.MANUFACTURER))
        // Absent rather than JSON null: the Rust side defaults the field to "unknown".
        DisplayKind.colorPanel()?.let { result.put("colorPanel", it) }
        invoke.resolve(result)
    }

    @Command
    fun getDeviceName(invoke: Invoke) {
        val manufacturer = Build.MANUFACTURER.replaceFirstChar { it.uppercase() }
        val model = Build.MODEL
        val name =
            if (model.startsWith(manufacturer, ignoreCase = true)) {
                model
            } else {
                "$manufacturer $model"
            }
        val result = JSObject()
        result.put("name", name)
        invoke.resolve(result)
    }
}
