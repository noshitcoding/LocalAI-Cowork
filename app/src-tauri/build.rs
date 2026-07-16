use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    prepare_webview2_loader();
    tauri_build::build()
}

fn prepare_webview2_loader() {
    println!("cargo:rerun-if-env-changed=CARGO_HOME");

    let manifest_dir = match env::var_os("CARGO_MANIFEST_DIR") {
        Some(value) => PathBuf::from(value),
        None => return,
    };
    let destination = manifest_dir.join("resources").join("WebView2Loader.dll");

    if destination.exists() {
        println!("cargo:rerun-if-changed={}", destination.display());
        return;
    }

    for candidate in webview2_loader_candidates() {
        if candidate.exists() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).expect("failed to create WebView2 resource directory");
            }
            fs::copy(&candidate, &destination).expect("failed to copy WebView2Loader.dll");
            println!("cargo:rerun-if-changed={}", candidate.display());
            return;
        }
    }

    panic!("WebView2Loader.dll not found in Cargo registry or build output");
}

fn webview2_loader_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(cargo_home) = cargo_home() {
        let registry_src = cargo_home.join("registry").join("src");
        push_registry_candidates(&registry_src, &mut candidates);
    }

    if let Some(out_dir) = env::var_os("OUT_DIR").map(PathBuf::from) {
        if let Some(profile_dir) = out_dir.ancestors().nth(3) {
            push_build_candidates(&profile_dir.join("build"), &mut candidates);
        }
    }

    candidates
}

fn cargo_home() -> Option<PathBuf> {
    env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".cargo")))
}

fn push_registry_candidates(registry_src: &Path, candidates: &mut Vec<PathBuf>) {
    let Ok(registries) = fs::read_dir(registry_src) else {
        return;
    };

    for registry in registries.flatten().filter(|entry| is_dir(entry.path())) {
        let Ok(crates) = fs::read_dir(registry.path()) else {
            continue;
        };

        for crate_dir in crates.flatten().filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("webview2-com-sys-")
        }) {
            candidates.push(crate_dir.path().join("x64").join("WebView2Loader.dll"));
        }
    }
}

fn push_build_candidates(build_dir: &Path, candidates: &mut Vec<PathBuf>) {
    let Ok(build_entries) = fs::read_dir(build_dir) else {
        return;
    };

    for entry in build_entries.flatten().filter(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with("webview2-com-sys-")
    }) {
        candidates.push(
            entry
                .path()
                .join("out")
                .join("x64")
                .join("WebView2Loader.dll"),
        );
    }
}

fn is_dir(path: PathBuf) -> bool {
    path.is_dir()
}
