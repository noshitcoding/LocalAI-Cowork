const COMMANDS: &[&str] = &["token", "permission_status", "request_permission", "consume_events"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
