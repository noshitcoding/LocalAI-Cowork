#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init());
    #[cfg(target_os = "android")]
    let builder = builder
        .plugin(tauri_plugin_biometric::init())
        .plugin(tauri_plugin_mobile_push::init())
        .plugin(tauri_plugin_mobile_secure::init());
    builder
        .run(tauri::generate_context!())
        .expect("error while running the Open Cowork Android shell");
}
