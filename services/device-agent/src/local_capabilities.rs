//! Discovery describes what this local profile can do; detection grants no rights.
use crate::execution::ExecutionConfig;
use serde_json::{Value, json};

pub fn describe(config: Option<&ExecutionConfig>) -> Value {
    let native = config.is_some_and(|c| c.native_owner_rights);
    let docker = config.and_then(|c| c.programs.get("docker"));
    let windows_sandbox = if cfg!(windows) {
        std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .map(|root| root.join("System32").join("WindowsSandbox.exe"))
            .is_some_and(|path| path.is_file())
    } else {
        false
    };
    json!({
        "format":"cowork-local-execution-capabilities/1",
        "platform":std::env::consts::OS,
        "selected_folders":{
            "file_operations":true,"native_shell_confined":false,
            "process_execution":"qualified_environment_required",
            "runtime_qualified":false
        },
        "user_rights":{
            "configured":native,"filesystem_isolation":false,
            "administrator_elevation":false,
            "local_consent_required":config.is_none_or(|c| c.require_local_consent),
            "configured_tools":config.map(|c| c.allowed_tools.clone()).unwrap_or_default()
        },
        "linux_container":{
            "docker_program_configured":docker.is_some(),
            "runtime_qualified":false,"native_windows_apps":false,
            "execution_path":"qualified_process_environment"
        },
        "windows_test_vm":{
            "host_supported":cfg!(windows),"launcher_installed":windows_sandbox,
            "runtime_qualified":false,"ephemeral":true,"max_concurrent_instances":1,
            "launch":"explicit_local_windows_sandbox_setup",
            "pairing":"separate_protocol_2_device_profile",
            "host_apps_available":false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detection_never_authorizes_native_or_claims_runtime_qualification() {
        let value = describe(None);
        assert_eq!(value["user_rights"]["configured"], false);
        assert_eq!(value["user_rights"]["filesystem_isolation"], false);
        assert_eq!(value["selected_folders"]["native_shell_confined"], false);
        assert_eq!(value["linux_container"]["runtime_qualified"], false);
        assert_eq!(value["windows_test_vm"]["runtime_qualified"], false);
        assert_eq!(value["windows_test_vm"]["host_apps_available"], false);
    }
    #[test]
    fn native_tool_names_cannot_bypass_independent_owner_permission() {
        let directory = tempfile::tempdir().unwrap();
        let config = ExecutionConfig {
            data_directory: directory.path().to_owned(),
            model: None,
            programs: Default::default(),
            environment: Default::default(),
            github_token: None,
            mcp_bindings: vec![],
            allowed_tools: vec!["bash".into()],
            native_owner_rights: false,
            require_local_consent: true,
            max_steps: 8,
            interop: Default::default(),
            capability_names: vec![],
        };
        let result = crate::execution::ExecutionEngine::open(config);
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("device_native_owner_rights_require_explicit_configuration")
        );
    }
}
