use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylusCapabilities {
    pub available: bool,
    pub pressure: bool,
    pub tilt: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafeAreaInsets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

/// Panel technology of the device. Decided natively: no CSS media feature reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayKind {
    Eink,
    Lcd,
    #[default]
    Unknown,
}

#[cfg(mobile)]
impl DisplayKind {
    fn from_native(kind: &str) -> Self {
        match kind {
            "eink" => Self::Eink,
            "lcd" => Self::Lcd,
            _ => Self::Unknown,
        }
    }
}

/// `color_panel` stays `None` unless the platform publishes a documented color-panel query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayInfo {
    pub kind: DisplayKind,
    pub color_panel: Option<bool>,
}

#[cfg(mobile)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeviceNameResponse {
    pub name: String,
}

#[cfg(mobile)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DisplayInfoResponse {
    pub kind: String,
    #[serde(default)]
    pub color_panel: Option<bool>,
}

#[cfg(mobile)]
impl From<DisplayInfoResponse> for DisplayInfo {
    fn from(response: DisplayInfoResponse) -> Self {
        Self {
            kind: DisplayKind::from_native(&response.kind),
            color_panel: response.color_panel,
        }
    }
}

#[cfg(mobile)]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemBarsStyleArgs {
    pub dark_background: bool,
}
