use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafeAreaInsets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

#[cfg(mobile)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeviceNameResponse {
    pub name: String,
}

#[cfg(mobile)]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemBarsStyleArgs {
    pub dark_background: bool,
}
