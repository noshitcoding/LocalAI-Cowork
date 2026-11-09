//! Pure private working-copy projection. These types are not API authority.
use crate::*;
use crate::{execution_inputs::CapturedExecutionInput, workspace_transfer as transfer};

pub const BROKER_TOOLS: &[&str] = &[
    "create_artifact",
    "list_artifacts",
    "read_artifact",
    "request_input",
    "list_files",
    "read_text_file",
    "stat_file",
    "write_text_file",
    "create_directory",
    "rename_file",
    "remove_file",
    "restore_file",
    "delegate_agents",
];
pub fn broker_scope(spec: &RunSpec) -> bool {
    spec.environment.is_none()
        && spec.executor == "local-v2"
        && spec.mcp_tools.is_empty()
        && spec.file_action.as_ref().is_none_or(|tool| {
            spec.provider.is_none()
                && spec.model_binding.is_none()
                && spec.workflow.is_none()
                && spec.agent_config.is_none()
                && spec.allowed_tools == [tool.name.as_str()]
                && match tool.name.as_str() {
                    "restore_file" => serde_json::from_value::<RestoreFile>(tool.arguments.clone())
                        .is_ok_and(|input| {
                            input
                                .expected_source_sha256
                                .as_deref()
                                .is_some_and(transfer::valid_sha256)
                                && transfer::validate_path(&input.path).is_ok()
                        }),
                    "import_file" => serde_json::from_value::<ImportFile>(tool.arguments.clone())
                        .is_ok_and(|input| {
                            transfer::valid_sha256(&input.file.sha256)
                                && input
                                    .expected_sha256
                                    .as_deref()
                                    .is_none_or(transfer::valid_sha256)
                                && transfer::validate_path(&input.path).is_ok()
                        }),
                    _ => false,
                }
        })
        && spec.allowed_tools.iter().all(|t| {
            BROKER_TOOLS.contains(&t.as_str())
                || t == "import_file"
                    && spec
                        .file_action
                        .as_ref()
                        .is_some_and(|tool| tool.name == "import_file")
        })
}

pub fn retains_workspace(spec: &RunSpec) -> bool {
    spec.input_workspace.is_some() || spec.folders.iter().any(|folder| folder.order_id.is_some())
}

/// A separate snapshot destination. This never widens the process-free broker
/// destination and does not authenticate a runner or qualify an environment.
pub fn managed_process_scope(spec: &RunSpec) -> bool {
    spec.environment.is_some()
        && spec.executor != "local-v2"
        && spec.executor != "owner-native-toolbox-v1"
        && spec.mcp_tools.is_empty()
        && spec.file_action.is_none()
        && spec
            .allowed_tools
            .iter()
            .any(|tool| tool == "execute_program")
        && spec
            .allowed_tools
            .iter()
            .all(|tool| BROKER_TOOLS.contains(&tool.as_str()) || tool == "execute_program")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FolderCopy {
    pub alias: String,
    pub grant: FolderGrant,
}

/// Persist this private intent before publication. It is not public authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunWorkspaceIntent {
    pub binding: RunWorkspaceBinding,
    pub project_id: String,
    pub order_id: String,
    pub nonce: String,
    pub copies: Vec<FolderCopy>,
}

fn input_reference(capture: &CapturedExecutionInput) -> ExecutionInputRef {
    ExecutionInputRef {
        snapshot_id: capture.snapshot.id.clone(),
        sha256: capture.sha256.clone(),
        manifest_sha256: capture.snapshot.manifest.sha256.clone(),
    }
}

impl RunWorkspaceIntent {
    pub fn for_spec(
        capture: &CapturedExecutionInput,
        source: &RunSpec,
        namespace: &str,
        nonce: &str,
    ) -> Result<Self> {
        capture.validate()?;
        uuid::Uuid::parse_str(namespace).map_err(|_| Error::Forbidden)?;
        uuid::Uuid::parse_str(nonce).map_err(|_| Error::Forbidden)?;
        if source.input_workspace.is_some()
            || source.project_id != capture.snapshot.project_id
            || source.order_id != capture.snapshot.order_id
        {
            return Err(Error::Forbidden);
        }
        let binding = RunWorkspaceBinding {
            id: namespace.into(),
            input: input_reference(capture),
        };
        let mut copies = Vec::new();
        for (origin, selected) in capture.origins.iter().zip(&capture.snapshot.sources) {
            let granted = source
                .folders
                .iter()
                .find(|g| g.id == origin.id)
                .ok_or(Error::Forbidden)?;
            let mut narrowed = origin.clone();
            narrowed.writable = granted.writable;
            if &narrowed != granted || granted.writable && !origin.writable {
                return Err(Error::Forbidden);
            }
            copies.push(FolderCopy {
                alias: selected.alias.clone(),
                grant: copy_grant(&binding, granted),
            });
        }
        if source
            .folders
            .iter()
            .any(|g| g.order_id.is_none() && !capture.origins.iter().any(|o| o.id == g.id))
        {
            return Err(Error::Forbidden);
        }
        // Do not implicitly import an unselected live project workspace.
        for managed in source.folders.iter().filter(|g| g.order_id.is_some()) {
            if managed.order_id.as_deref() != Some(source.order_id.as_str()) {
                return Err(Error::Forbidden);
            }
            if !copies.iter().any(|c| c.grant.id == managed.id) {
                if copies.iter().any(|c| c.alias == "output") {
                    return Err(Error::Forbidden);
                }
                copies.push(FolderCopy {
                    alias: "output".into(),
                    grant: copy_grant(&binding, managed),
                });
            }
        }
        let intent = Self {
            binding,
            project_id: source.project_id.clone(),
            order_id: source.order_id.clone(),
            nonce: nonce.into(),
            copies,
        };
        let output_entries = usize::from(intent.copies.iter().any(|c| c.alias == "output"));
        if capture
            .snapshot
            .manifest
            .directories
            .len()
            .checked_add(capture.snapshot.manifest.files.len())
            .and_then(|n| n.checked_add(output_entries))
            .is_none_or(|n| n > transfer::MAX_ENTRIES)
        {
            return Err(Error::Validation("run_workspace_capacity_exceeded".into()));
        }
        intent.validate()?;
        Ok(intent)
    }

    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.binding.id,
            &self.nonce,
            &self.binding.input.snapshot_id,
        ] {
            uuid::Uuid::parse_str(id).map_err(|_| Error::Forbidden)?;
        }
        if !transfer::valid_sha256(&self.binding.input.sha256)
            || !transfer::valid_sha256(&self.binding.input.manifest_sha256)
            || self.copies.len() > 33
        {
            return Err(Error::Forbidden);
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut aliases = std::collections::BTreeSet::new();
        for copy in &self.copies {
            transfer::validate_path(&copy.alias)?;
            if copy.alias.contains('/')
                || !ids.insert(&copy.grant.id)
                || !aliases.insert(&copy.alias)
                || copy.grant.project_id != self.project_id
                || copy
                    .grant
                    .order_id
                    .as_ref()
                    .is_some_and(|o| o != &self.order_id)
                || copy.grant != copy_grant(&self.binding, &copy.grant)
            {
                return Err(Error::Forbidden);
            }
        }
        Ok(())
    }
    pub fn folders(&self) -> Vec<FolderGrant> {
        self.copies.iter().map(|c| c.grant.clone()).collect()
    }
}

fn copy_grant(binding: &RunWorkspaceBinding, source: &FolderGrant) -> FolderGrant {
    let mut grant = source.clone();
    grant.resource_id = canonical_hash(
        &json!({"domain":"run-workspace-resource/1","namespace":binding.id,"folder_id":source.id}),
    );
    grant.revision = canonical_hash(
        &json!({"domain":"run-workspace-revision/1","binding":binding,"folder_id":source.id,
        "project_id":source.project_id,"order_id":source.order_id,"name":source.name,"writable":source.writable}),
    );
    grant
}
