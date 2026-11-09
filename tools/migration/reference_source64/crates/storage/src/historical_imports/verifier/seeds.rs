pub(super) fn source_seed(name: &str) -> Option<&'static [u8]> {
    match name {
        "desktop-0-uninitialized" => Some(include_bytes!("data/desktop-0-uninitialized.sqlite3")),
        "desktop-0" => Some(include_bytes!("data/desktop-0.sqlite3")),
        "desktop-1" => Some(include_bytes!("data/desktop-1.sqlite3")),
        "desktop-2" => Some(include_bytes!("data/desktop-2.sqlite3")),
        "desktop-3" => Some(include_bytes!("data/desktop-3.sqlite3")),
        "desktop-4" => Some(include_bytes!("data/desktop-4.sqlite3")),
        "desktop-5" => Some(include_bytes!("data/desktop-5.sqlite3")),
        "desktop-6" => Some(include_bytes!("data/desktop-6.sqlite3")),
        "desktop-7" => Some(include_bytes!("data/desktop-7.sqlite3")),
        "desktop-8" => Some(include_bytes!("data/desktop-8.sqlite3")),
        "desktop-9" => Some(include_bytes!("data/desktop-9.sqlite3")),
        "desktop-10" => Some(include_bytes!("data/desktop-10.sqlite3")),
        "desktop-11" => Some(include_bytes!("data/desktop-11.sqlite3")),
        "desktop-12" => Some(include_bytes!("data/desktop-12.sqlite3")),
        "desktop-13" => Some(include_bytes!("data/desktop-13.sqlite3")),
        "desktop-14" => Some(include_bytes!("data/desktop-14.sqlite3")),
        "desktop-15" => Some(include_bytes!("data/desktop-15.sqlite3")),
        "desktop-16" => Some(include_bytes!("data/desktop-16.sqlite3")),
        "desktop-17" => Some(include_bytes!("data/desktop-17.sqlite3")),
        "desktop-18" => Some(include_bytes!("data/desktop-18.sqlite3")),
        "desktop-19" => Some(include_bytes!("data/desktop-19.sqlite3")),
        "desktop-20" => Some(include_bytes!("data/desktop-20.sqlite3")),
        "desktop-21" => Some(include_bytes!("data/desktop-21.sqlite3")),
        "desktop-22" => Some(include_bytes!("data/desktop-22.sqlite3")),
        "desktop-23" => Some(include_bytes!("data/desktop-23.sqlite3")),
        "desktop-24" => Some(include_bytes!("data/desktop-24.sqlite3")),
        "desktop-25" => Some(include_bytes!("data/desktop-25.sqlite3")),
        "desktop-26" => Some(include_bytes!("data/desktop-26.sqlite3")),
        "desktop-27" => Some(include_bytes!("data/desktop-27.sqlite3")),
        "desktop-27-backend-selection" => {
            Some(include_bytes!("data/desktop-27-backend-selection.sqlite3"))
        }
        "desktop-28" => Some(include_bytes!("data/desktop-28.sqlite3")),
        "desktop-28-backend-selection" => {
            Some(include_bytes!("data/desktop-28-backend-selection.sqlite3"))
        }
        "daemon-base-13" => Some(include_bytes!("data/daemon-base-13.sqlite3")),
        "daemon-sync-15" => Some(include_bytes!("data/daemon-sync-15.sqlite3")),
        _ => None,
    }
}
