#[cfg(target_os = "linux")]
fn main() {
    std::process::exit(cowork_managed_runtime::containerd::deadline_process::entrypoint());
}
#[cfg(not(target_os = "linux"))]
fn main() {
    std::process::exit(2);
}
