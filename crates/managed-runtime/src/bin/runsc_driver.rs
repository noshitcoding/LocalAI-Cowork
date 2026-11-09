#[cfg(all(target_os = "linux", feature = "containerd"))]
fn main() {
    std::process::exit(cowork_managed_runtime::containerd::runsc_driver::entrypoint());
}
#[cfg(not(all(target_os = "linux", feature = "containerd")))]
fn main() {
    std::process::exit(1);
}
