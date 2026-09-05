package dev.okhsunrog.tangleaf

import android.graphics.Color
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.WindowCompat

class MainActivity : TauriActivity() {
  init {
    System.loadLibrary("tangleaf_lib")
  }

  private external fun initializeRustlsPlatformVerifier(context: android.content.Context): Boolean

  override fun onCreate(savedInstanceState: Bundle?) {
    check(initializeRustlsPlatformVerifier(applicationContext)) {
      "Failed to initialize the Android TLS certificate verifier"
    }
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    WindowCompat.setDecorFitsSystemWindows(window, false)
    window.statusBarColor = Color.TRANSPARENT
    window.navigationBarColor = Color.TRANSPARENT
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      window.isStatusBarContrastEnforced = false
      window.isNavigationBarContrastEnforced = false
    }
  }
}
