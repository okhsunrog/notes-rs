const COMMANDS: &[&str] = &[
    "get_safe_area_insets",
    "get_device_name",
    "set_system_bars_style",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
