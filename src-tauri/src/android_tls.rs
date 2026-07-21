use jni::{
    EnvUnowned,
    errors::ThrowRuntimeExAndDefault,
    objects::JObject,
    sys::{JNI_TRUE, jboolean},
};

/// Initializes Android's certificate verifier before Tauri starts any networking.
///
/// This is called synchronously from `MainActivity.onCreate`. The explicit JNI entrypoint is
/// intentional: Tauri's Rust setup hook runs after the Android activity and does not expose the
/// `JNIEnv` and `Context` handles required by rustls-platform-verifier.
#[unsafe(export_name = "Java_dev_okhsunrog_notes_1rs_MainActivity_initializeRustlsPlatformVerifier")]
pub extern "system" fn initialize_rustls_platform_verifier<'caller>(
    mut env: EnvUnowned<'caller>,
    _activity: JObject<'caller>,
    context: JObject<'caller>,
) -> jboolean {
    env.with_env(|env| -> Result<jboolean, jni::errors::Error> {
        rustls_platform_verifier::android::init_with_env(env, context)?;
        Ok(JNI_TRUE)
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}
