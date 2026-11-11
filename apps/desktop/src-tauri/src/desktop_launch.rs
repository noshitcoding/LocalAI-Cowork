//! Immutable local launch profile. No global environment or startup entry is changed here.
use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
};
const ARG: &str = "--cowork-data-dir";
#[derive(Clone, Debug, Default)]
pub struct DataDirectory {
    value: Option<PathBuf>,
}
impl DataDirectory {
    pub fn from_process() -> Result<Self, String> {
        Self::parse(
            std::env::args_os().skip(1),
            std::env::var_os("COWORK_DATA_DIR"),
        )
    }
    pub fn parse(
        args: impl IntoIterator<Item = OsString>,
        environment: Option<OsString>,
    ) -> Result<Self, String> {
        let args: Vec<_> = args.into_iter().collect();
        let argument = match args.as_slice() {
            [] => None,
            [flag, value] if flag == ARG => Some(valid_path(value)?),
            _ => return Err("invalid_desktop_launch_arguments".into()),
        };
        // Preserve the established launcher environment precedence. Both sources are
        // validated first: malformed CLI/environment never silently selects a profile.
        let value = environment
            .as_deref()
            .map(valid_path)
            .transpose()?
            .or(argument);
        Ok(Self { value })
    }
    pub fn path(&self) -> Option<&Path> {
        self.value.as_deref()
    }
    pub fn autostart_args(&self) -> Vec<String> {
        self.value.as_ref().map_or_else(Vec::new, |path| {
            vec![
                ARG.into(),
                path.to_str().expect("validated launch path").into(),
            ]
        })
    }
}
/// Bind explicit launch profiles before Tauri creates configured windows.
pub fn configure_webviews(
    config: &mut tauri::utils::config::Config,
    directory: Option<&Path>,
) -> Result<(), String> {
    use tauri::utils::config::{AppDirectoriesOverride, AppDirectoryOverrides};
    let Some(directory) = directory else {
        return Ok(());
    };
    let webview = valid_path(directory.as_os_str())?.join("webview");
    let mut directories = match config.app.app_directories_override.take() {
        Some(AppDirectoriesOverride::Directories(directories)) => directories,
        Some(AppDirectoriesOverride::Root(root)) => AppDirectoryOverrides {
            config: Some(root.clone()),
            data: Some(root.clone()),
            local_data: None,
            cache: Some(root.join("caches")),
            log: Some(root.join("logs")),
        },
        None => AppDirectoryOverrides::default(),
    };
    directories.local_data = Some(webview);
    config.app.app_directories_override = Some(AppDirectoriesOverride::Directories(directories));
    // WindowConfig.data_directory is relative to the OS LocalAppData directory,
    // not this override. Unset it so every configured WebView uses our absolute
    // app-local override without invoking Tauri's KnownFolder default closure.
    for window in &mut config.app.windows {
        window.data_directory = None;
    }
    Ok(())
}
fn valid_path(value: &std::ffi::OsStr) -> Result<PathBuf, String> {
    let text = value.to_str().ok_or("invalid_desktop_data_path")?;
    let path = PathBuf::from(value);
    if text.is_empty()
        || text.len() > 8000
        || text.chars().any(|c| c.is_control() || c == '"')
        || !path.is_absolute()
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err("invalid_desktop_data_path".into());
    }
    #[cfg(windows)]
    if !matches!(path.components().next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),std::path::Prefix::Disk(_)|std::path::Prefix::UNC(_,_)))
    {
        return Err("invalid_desktop_data_path".into());
    }
    Ok(path)
}
#[cfg(windows)]
pub fn startup_command(executable: &Path, directory: &Path) -> Result<String, String> {
    let executable = valid_path(executable.as_os_str())?;
    let directory = valid_path(directory.as_os_str())?;
    let args = [
        executable.to_str().unwrap(),
        ARG,
        directory.to_str().unwrap(),
    ];
    Ok(args
        .into_iter()
        .map(windows_argument)
        .collect::<Vec<_>>()
        .join(" "))
}
#[cfg(windows)]
fn windows_argument(value: &str) -> String {
    // Windows argv encoding, not cmd.exe/PowerShell syntax. Double trailing
    // backslashes before the closing quote and escape embedded quote boundaries.
    let mut result = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            '\\',
            if c == '"' { slashes * 2 + 1 } else { slashes },
        ));
        slashes = 0;
        result.push(c);
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    fn path(name: &str) -> OsString {
        #[cfg(windows)]
        let root = r"C:\Users\Test\LocalAI Cowork";
        #[cfg(not(windows))]
        let root = "/home/test/LocalAI Cowork";
        Path::new(root).join(name).into_os_string()
    }
    fn webview_config() -> tauri::utils::config::Config {
        serde_json::from_value(serde_json::json!({
            "identifier": "dev.localai.cowork.v2.preview",
            "app": { "windows": [
                { "label": "main" },
                { "label": "other", "dataDirectory": "old-relative-profile", "create": false }
            ] }
        }))
        .unwrap()
    }
    #[test]
    fn default_launch_preserves_all_existing_webview_and_app_directory_configuration() {
        let mut config = webview_config();
        config.app.app_directories_override = Some(
            tauri::utils::config::AppDirectoriesOverride::Root(PathBuf::from(path("existing"))),
        );
        let before = serde_json::to_value(&config).unwrap();
        configure_webviews(&mut config, None).unwrap();
        assert_eq!(serde_json::to_value(config).unwrap(), before);
    }
    #[test]
    fn explicit_profile_routes_every_configured_window_to_its_private_webview_directory() {
        use tauri::utils::config::{AppDirectoriesOverride, AppDirectoryOverrides};
        let directory = PathBuf::from(path("Daten ä & spaces"));
        let mut config = webview_config();
        let original = config.app.windows.clone();
        configure_webviews(&mut config, Some(&directory)).unwrap();
        assert_eq!(
            config.app.app_directories_override,
            Some(AppDirectoriesOverride::Directories(AppDirectoryOverrides {
                local_data: Some(directory.join("webview")),
                ..Default::default()
            }))
        );
        let mut expected = original;
        for window in &mut expected {
            window.data_directory = None;
        }
        assert_eq!(config.app.windows, expected);
    }
    #[test]
    fn private_webview_override_preserves_other_individual_app_directory_overrides() {
        use tauri::utils::config::{AppDirectoriesOverride, AppDirectoryOverrides};
        let mut config = webview_config();
        let mut expected = AppDirectoryOverrides {
            config: Some("$CONFIG/keep-config".into()),
            data: Some("$DATA/keep-data".into()),
            local_data: Some("old-webview".into()),
            cache: Some("$CACHE/keep-cache".into()),
            log: Some("relative-logs".into()),
        };
        config.app.app_directories_override =
            Some(AppDirectoriesOverride::Directories(expected.clone()));
        let directory = PathBuf::from(path("private"));
        configure_webviews(&mut config, Some(&directory)).unwrap();
        expected.local_data = Some(directory.join("webview"));
        assert_eq!(
            config.app.app_directories_override,
            Some(AppDirectoriesOverride::Directories(expected))
        );
    }
    #[test]
    fn private_webview_override_preserves_the_other_root_override_locations() {
        use tauri::utils::config::{AppDirectoriesOverride, AppDirectoryOverrides};
        let root = PathBuf::from(path("other-app-root"));
        let directory = PathBuf::from(path("explicit-profile"));
        let mut config = webview_config();
        config.app.app_directories_override = Some(AppDirectoriesOverride::Root(root.clone()));
        configure_webviews(&mut config, Some(&directory)).unwrap();
        assert_eq!(
            config.app.app_directories_override,
            Some(AppDirectoriesOverride::Directories(AppDirectoryOverrides {
                config: Some(root.clone()),
                data: Some(root.clone()),
                local_data: Some(directory.join("webview")),
                cache: Some(root.join("caches")),
                log: Some(root.join("logs")),
            }))
        );
    }
    #[test]
    fn environment_and_startup_argument_select_the_same_isolated_webview_profile() {
        let selected = path("private ä");
        let environment = DataDirectory::parse([], Some(selected)).unwrap();
        let startup = DataDirectory::parse(
            environment.autostart_args().into_iter().map(OsString::from),
            None,
        )
        .unwrap();
        let mut first = webview_config();
        let mut second = webview_config();
        configure_webviews(&mut first, environment.path()).unwrap();
        configure_webviews(&mut second, startup.path()).unwrap();
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&second).unwrap()
        );
        let other = PathBuf::from(path("another-profile"));
        configure_webviews(&mut second, Some(&other)).unwrap();
        assert_ne!(
            first.app.app_directories_override,
            second.app.app_directories_override
        );
    }
    #[test]
    fn invalid_webview_profile_is_rejected_before_configuration_changes() {
        for directory in [
            PathBuf::from("relative"),
            PathBuf::from(path("safe")).join(".."),
        ] {
            let mut config = webview_config();
            let before = serde_json::to_value(&config).unwrap();
            assert!(configure_webviews(&mut config, Some(&directory)).is_err());
            assert_eq!(serde_json::to_value(config).unwrap(), before);
        }
    }
    #[test]
    fn custom_environment_and_startup_argument_reopen_exact_same_profile_without_global_env() {
        let selected = path("Daten ä & $literal");
        let config = DataDirectory::parse([], Some(selected.clone())).unwrap();
        assert_eq!(config.path(), Some(Path::new(&selected)));
        let args = config.autostart_args();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], ARG);
        let reopened = DataDirectory::parse(args.into_iter().map(OsString::from), None).unwrap();
        assert_eq!(reopened.path(), config.path());
        assert!(DataDirectory::parse([], None).unwrap().path().is_none());
    }
    #[test]
    fn valid_environment_precedes_valid_cli_but_never_hides_malformed_input() {
        let environment = path("Environment");
        let cli = path("Argument");
        let args = vec![ARG.into(), cli];
        let selected = DataDirectory::parse(args.clone(), Some(environment.clone())).unwrap();
        assert_eq!(selected.path(), Some(Path::new(&environment)));
        assert!(DataDirectory::parse(vec![ARG.into()], Some(environment)).is_err());
        assert!(DataDirectory::parse(args, Some("relative".into())).is_err());
    }
    #[test]
    fn malformed_relative_duplicate_and_reserved_mode_arguments_fail_closed() {
        for args in [
            vec![ARG.into()],
            vec![ARG.into(), "relative".into()],
            vec![ARG.into(), path("one"), ARG.into(), path("two")],
            vec!["--cowork-data-dir=relative".into()],
            vec!["--cowork-update-exit-witness".into()],
            vec!["--cowork-installer-observer".into()],
        ] {
            assert!(DataDirectory::parse(args, None).is_err());
        }
        for invalid in ["", "relative", "bad\0path", "bad\npath"] {
            assert!(DataDirectory::parse([], Some(invalid.into())).is_err());
        }
        assert!(
            DataDirectory::parse(
                [],
                Some(
                    Path::new(&path("one"))
                        .join("..")
                        .join("two")
                        .into_os_string()
                )
            )
            .is_err()
        );
        #[cfg(windows)]
        for invalid in [
            r"C:relative",
            r"\root-relative",
            r"\\?\C:\Data",
            r#"C:\bad\"quote"#,
        ] {
            assert!(DataDirectory::parse([], Some(invalid.into())).is_err());
        }
    }
    #[cfg(windows)]
    #[test]
    fn windows_startup_command_roundtrips_spaces_unicode_metacharacters_and_trailing_slashes() {
        use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
        let executable =
            Path::new(r"C:\Program Files\LocalAI Cowork V2\localai-cowork-v2-desktop.exe");
        for directory in [
            r"C:\Users\Test\Daten ä & $value`literal\",
            r"\\host\share\LocalAI Daten\",
        ] {
            let command = startup_command(executable, Path::new(directory)).unwrap();
            let wide: Vec<_> = command.encode_utf16().chain(Some(0)).collect();
            let mut count = 0;
            let parsed = unsafe { CommandLineToArgvW(wide.as_ptr(), &mut count) };
            assert!(!parsed.is_null());
            let args: Vec<String> = (0..count as usize)
                .map(|i| unsafe {
                    let ptr = *parsed.add(i);
                    let mut n = 0;
                    while *ptr.add(n) != 0 {
                        n += 1;
                    }
                    String::from_utf16(std::slice::from_raw_parts(ptr, n)).unwrap()
                })
                .collect();
            unsafe {
                LocalFree(parsed.cast());
            }
            assert_eq!(args, vec![executable.to_str().unwrap(), ARG, directory]);
            let reopened =
                DataDirectory::parse(args.into_iter().skip(1).map(OsString::from), None).unwrap();
            assert_eq!(reopened.path(), Some(Path::new(directory)));
        }
    }
}
