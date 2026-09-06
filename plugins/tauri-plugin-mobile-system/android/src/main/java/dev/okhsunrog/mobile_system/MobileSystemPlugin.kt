package dev.okhsunrog.mobile_system

import android.app.Activity
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.hardware.input.InputManager
import android.view.InputDevice
import android.view.MotionEvent
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
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

@TauriPlugin
class MobileSystemPlugin(private val activity: Activity) : Plugin(activity), InputManager.InputDeviceListener {
    private val inputManager = activity.getSystemService(InputManager::class.java)
    private var inkWebView: WebView? = null
    private var onyxInk: OnyxInk? = null

    override fun load(webView: WebView) {
        inkWebView = webView
        inputManager.registerInputDeviceListener(this, Handler(Looper.getMainLooper()))
    }

    @Suppress("OVERRIDE_DEPRECATION") // This plugin does not depend on AppCompat types.
    override fun onDestroy() {
        onyxInk?.destroy()
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
                if (onyxInk == null && args.enabled) {
                    onyxInk = OnyxInk(activity, checkNotNull(inkWebView)) { trigger("onyxInk", it) }
                }
                invoke.resolve(onyxInk?.configure(args) ?: JSObject().put("available", true).put("active", false))
            } catch (error: Throwable) {
                invoke.reject("Could not start BOOX ink: ${error.message}")
            }
        }
    }

    @Command
    fun commitOnyxFrame(invoke: Invoke) {
        val args = invoke.parseArgs(OnyxFrameArgs::class.java)
        activity.runOnUiThread {
            onyxInk?.commit(args)
            invoke.resolve()
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
