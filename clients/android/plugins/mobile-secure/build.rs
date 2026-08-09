const COMMANDS: &[&str] = &["store", "retrieve", "remove"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
