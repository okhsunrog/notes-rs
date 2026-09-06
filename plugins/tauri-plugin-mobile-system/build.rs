const COMMANDS: &[&str] = &[
    "configure_onyx_ink",
    "commit_onyx_frame",
    "suppress_onyx_ink",
    "get_stylus_capabilities",
    "get_safe_area_insets",
    "get_device_name",
    "set_system_bars_style",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
