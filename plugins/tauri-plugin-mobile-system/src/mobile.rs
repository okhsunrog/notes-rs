use serde::de::DeserializeOwned;
use tauri::{
    AppHandle, Runtime,
    plugin::{PluginApi, PluginHandle},
};

use crate::{
    Result,
    models::{DeviceNameResponse, SafeAreaInsets, SystemBarsStyleArgs},
};

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "dev.okhsunrog.mobile_system";

pub struct MobileSystem<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> MobileSystem<R> {
    pub fn stylus_capabilities(&self) -> Result<crate::StylusCapabilities> {
        self.0
            .run_mobile_plugin("getStylusCapabilities", ())
            .map_err(Into::into)
    }

    pub fn safe_area_insets(&self) -> Result<SafeAreaInsets> {
        self.0
            .run_mobile_plugin("getSafeAreaInsets", ())
            .map_err(Into::into)
    }

    pub fn device_name(&self) -> Result<String> {
        self.0
            .run_mobile_plugin::<DeviceNameResponse>("getDeviceName", ())
            .map(|response| response.name)
            .map_err(Into::into)
    }

    pub fn set_system_bars_style(&self, dark_background: bool) -> Result<()> {
        self.0
            .run_mobile_plugin(
                "setSystemBarsStyle",
                SystemBarsStyleArgs { dark_background },
            )
            .map_err(Into::into)
    }
}

#[cfg(target_os = "android")]
pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> Result<MobileSystem<R>> {
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "MobileSystemPlugin")?;
    Ok(MobileSystem(handle))
}

#[cfg(target_os = "ios")]
pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> Result<MobileSystem<R>> {
    Err(crate::Error::Platform(
        "the iOS implementation has not been added yet".into(),
    ))
}
