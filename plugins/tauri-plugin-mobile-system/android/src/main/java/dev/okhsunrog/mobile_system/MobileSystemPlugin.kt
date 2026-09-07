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
import com.onyx.android.sdk.api.device.epd.UpdateOption
import com.onyx.android.sdk.api.device.eac.EACReflectUtils
import com.onyx.android.sdk.utils.ReflectUtil
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
    private val fullRefresh = Runnable {
        inkWebView?.let { view ->
            runCatching { EpdController.invalidate(view, UpdateMode.GC) }
                .onFailure { Log.w("OnyxInk", "full refresh: ${it.message}") }
        }
    }

    /** One GC refresh after the screen settles; repeated requests collapse into the last one. */
    internal fun fullRefreshSoon(webView: WebView) {
        webView.removeCallbacks(fullRefresh)
        webView.postDelayed(fullRefresh, FULL_REFRESH_DELAY_MS)
    }

    private fun imeChanged(webView: WebView, visible: Boolean) {
        if (visible == imeVisible) return
        imeVisible = visible
        Log.d("OnyxInk", "ime visible=$visible")
        onyxInk?.imeChanged(visible)
        // Typing under the partial modes leaves the caret's trail behind; the keyboard going
        // away is the natural moment to clean the panel once, not on every keystroke.
        if (!visible && OnyxInk.supported()) fullRefreshSoon(webView)
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
            .put("appRefreshMode", applyAppRefreshProfile())
            .also { ensureSpeedRefreshProfile() }
    }

    /**
     * Writes the Speed refresh profile into this app's EinkWise (EAC) configuration, the same
     * way the EinkWise panel does: fetch the app's config JSON from the optimisation service,
     * change the refresh block, hand it back. The runtime `setAppScopeRefreshMode` had no effect
     * on this firmware; the stored profile is what the panel actually obeys for the caret,
     * touches and scrolling. Idempotent, off the UI thread, and a no-op when the service or the
     * reflection hooks are missing.
     */
    private fun ensureSpeedRefreshProfile() {
        val get = EACReflectUtils.sMethodGetAppConfigFromService ?: return
        val apply = EACReflectUtils.sMethodApplyAppConfigToService ?: return
        val pkg = activity.packageName
        Thread {
            runCatching {
                val configs = ReflectUtil.invokeMethodSafely(get, null, listOf(pkg)) as? List<*>
                val json = configs?.firstOrNull() as? String
                if (json.isNullOrBlank()) {
                    Log.w("OnyxInk", "eac: no app config returned for $pkg")
                    return@runCatching
                }
                val root = org.json.JSONObject(json)
                val refresh = root.getJSONObject("globalActivityConfig").getJSONObject("refreshConfig")
                val current = refresh.optString("refreshModeIndex")
                if (current == SPEED_MODE_INDEX && refresh.optInt("updateMode") == SPEED_UPDATE_MODE) {
                    Log.d("OnyxInk", "eac: refresh profile already Speed")
                    return@runCatching
                }
                refresh.put("refreshModeIndex", SPEED_MODE_INDEX)
                refresh.put("updateMode", SPEED_UPDATE_MODE)
                refresh.put("turbo", SPEED_TURBO)
                refresh.remove("refreshModeAlias")
                val bundle = android.os.Bundle().apply { putInt("args_operation_flag", 0) }
                val result = ReflectUtil.invokeMethodSafely(apply, null, listOf(root.toString()), bundle)
                Log.d("OnyxInk", "eac: refresh profile $current -> $SPEED_MODE_INDEX result=$result")
            }.onFailure { Log.w("OnyxInk", "eac: refresh profile: ${it.message}") }
        }.start()
    }

    /**
     * The firmware refreshes the caret, touches and scrolling by the app's refresh profile — the
     * one EinkWise shows as HD / Balanced / Regal / Speed. Regal turns each of those into a full
     * flash; Speed keeps them partial, which is what a note app wants. The SDK exposes the
     * profile at runtime through `setAppScopeRefreshMode`, but if the firmware lacks the app-scope
     * method the SDK silently falls back to the *system* profile, so the method is probed first.
     * Runtime only: EinkWise's stored profile stays whatever the user chose.
     */
    private fun applyAppRefreshProfile(): String? {
        val supported = runCatching {
            Class.forName("android.onyx.optimization.EInkHelper")
                .getMethod("setAppScopeRefreshMode", Int::class.javaPrimitiveType)
        }.isSuccess
        if (!supported) {
            Log.w("OnyxInk", "app-scope refresh mode unsupported by this firmware; leaving EinkWise's profile")
            return null
        }
        // The SDK reports NORMAL for any reflection failure, so the firmware calls are made and
        // logged directly here as well; the raw integers are what the panel actually holds.
        val helper = runCatching { Class.forName("android.onyx.optimization.EInkHelper") }.getOrNull()
        val rawBefore = helper?.let { readAppScopeRaw(it) }
        return runCatching {
            EpdController.setAppScopeRefreshMode(UpdateOption.FAST)
            val applied = EpdController.getAppScopeRefreshMode()
            val rawAfter = helper?.let { readAppScopeRaw(it) }
            Log.d("OnyxInk", "app-scope refresh mode requested=FAST applied=$applied raw before=$rawBefore after=$rawAfter")
            applied?.name
        }.onFailure { Log.w("OnyxInk", "app-scope refresh mode: ${it.message}") }.getOrNull()
    }

    private fun readAppScopeRaw(helper: Class<*>): String = runCatching {
        val method = helper.getMethod("getAppScopeRefreshMode")
        method.invoke(null).toString()
    }.getOrElse { "error: ${it.javaClass.simpleName}: ${it.message ?: it.cause?.message}" }

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

    private companion object {
        const val FULL_REFRESH_DELAY_MS = 300L
        /** EinkWise "Speed": partial GU updates with turbo, as the stock browser profile has it. */
        const val SPEED_MODE_INDEX = "refresh_mode_2"
        const val SPEED_UPDATE_MODE = 2
        const val SPEED_TURBO = 5
    }
}
