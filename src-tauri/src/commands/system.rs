use super::*;

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Native enumeration is currently implemented on Android only.
pub enum StylusAvailability {
    Available,
    NotDetected,
    Unknown,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct InputCapabilities {
    pub stylus: StylusAvailability,
    pub pressure: bool,
    pub tilt: bool,
    pub native_device_events: bool,
}

#[tauri::command]
#[specta::specta]
pub fn input_capabilities(app: AppHandle) -> CommandResult<InputCapabilities> {
    #[cfg(target_os = "android")]
    {
        let native = app.mobile_system().stylus_capabilities().map_err(err)?;
        Ok(InputCapabilities {
            stylus: if native.available {
                StylusAvailability::Available
            } else {
                StylusAvailability::NotDetected
            },
            pressure: native.pressure,
            tilt: native.tilt,
            native_device_events: true,
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(InputCapabilities {
            stylus: StylusAvailability::Unknown,
            pressure: false,
            tilt: false,
            native_device_events: false,
        })
    }
}

/// Panel technology reported by the platform. There is no CSS media feature for it: `(update: slow)`
/// is not reported by the e-ink WebView, so the frontend display profile depends on this value.
#[derive(Debug, Clone, Copy, Default, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Native detection is currently implemented on Android only.
pub enum DisplayKind {
    Eink,
    Lcd,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MobileSystemInfo {
    pub device_name: String,
    pub safe_area: SafeAreaInsets,
    pub display_kind: DisplayKind,
    /// `None` while no platform publishes a documented color-panel query.
    pub color_panel: Option<bool>,
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
        let display = integration.display_info().map_err(err)?;
        Ok(Some(MobileSystemInfo {
            device_name: integration.device_name().map_err(err)?,
            safe_area: SafeAreaInsets {
                top: native.top,
                right: native.right,
                bottom: native.bottom,
                left: native.left,
            },
            display_kind: match display.kind {
                tauri_plugin_mobile_system::DisplayKind::Eink => DisplayKind::Eink,
                tauri_plugin_mobile_system::DisplayKind::Lcd => DisplayKind::Lcd,
                tauri_plugin_mobile_system::DisplayKind::Unknown => DisplayKind::Unknown,
            },
            color_panel: display.color_panel,
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
