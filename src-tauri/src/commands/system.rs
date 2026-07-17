use super::*;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MobileSystemInfo {
    pub device_name: String,
    pub safe_area: SafeAreaInsets,
}

#[derive(Debug, Clone, Copy, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SafeAreaInsets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

#[tauri::command]
#[specta::specta]
pub fn mobile_system_info(app: AppHandle) -> CommandResult<Option<MobileSystemInfo>> {
    #[cfg(target_os = "android")]
    {
        let integration = app.mobile_system();
        let native = integration.safe_area_insets().map_err(err)?;
        Ok(Some(MobileSystemInfo {
            device_name: integration.device_name().map_err(err)?,
            safe_area: SafeAreaInsets {
                top: native.top,
                right: native.right,
                bottom: native.bottom,
                left: native.left,
            },
        }))
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(None)
    }
}

#[tauri::command]
#[specta::specta]
pub fn set_system_bars_style(app: AppHandle, dark_background: bool) -> CommandResult<()> {
    #[cfg(target_os = "android")]
    {
        app.mobile_system()
            .set_system_bars_style(dark_background)
            .map_err(err)
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, dark_background);
        Ok(())
    }
}
