# Public API is reached through Tauri's generated mobile plugin bindings.
# The vendor SDK uses reflection and EventBus subscribers internally.
-keep class com.onyx.android.sdk.** { *; }
