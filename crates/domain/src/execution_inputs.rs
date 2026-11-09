//! Local custody only. Native source grants never enter the portable snapshot.
use crate::*;
use std::collections::BTreeSet;

pub const MAX_INPUT_SOURCES: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedExecutionInput {
    pub snapshot: ExecutionInputSnapshot,
    pub sha256: String,
    /// Local baseline/capabilities, not an externally accepted authority claim.
    pub origins: Vec<FolderGrant>,
}

pub fn validate_request(request: &PrepareExecutionInput) -> Result<()> {
    valid_text(&request.idempotency_key, 128)?;
    if request.base_revision <= 0 || request.folder_ids.len() > MAX_INPUT_SOURCES {
        return Err(Error::Validation("execution_input_selection_limit".into()));
    }
    let mut previous = "";
    for folder in &request.folder_ids {
        valid_text(folder, 256)?;
        if folder.as_str() <= previous {
            return Err(Error::Validation("execution_input_selection_order".into()));
        }
        previous = folder;
    }
    Ok(())
}

pub fn request_hash(order: &str, request: &PrepareExecutionInput) -> String {
    canonical_hash(
        &json!({"domain":"execution-input-request/1","order_id":order,"request":request}),
    )
}

pub fn source_alias(index: usize) -> String {
    format!("input-{:04}", index + 1)
}

impl CapturedExecutionInput {
    pub fn empty(
        order: &Order,
        request: &PrepareExecutionInput,
        snapshot_id: &str,
        at: i64,
    ) -> Result<Self> {
        if !request.folder_ids.is_empty() || order.revision != request.base_revision {
            return Err(Error::Conflict);
        }
        let snapshot = ExecutionInputSnapshot {
            format_version: 1,
            id: snapshot_id.into(),
            order_id: order.id.clone(),
            project_id: order.project_id.clone(),
            order_revision: order.revision,
            request_sha256: request_hash(&order.id, request),
            created_at: at,
            sources: vec![],
            manifest: workspace_transfer::build(vec![], vec![])?,
        };
        let captured = Self {
            sha256: canonical_hash(&snapshot),
            snapshot,
            origins: vec![],
        };
        captured.validate_request(request)?;
        Ok(captured)
    }

    pub fn validate(&self) -> Result<()> {
        let snapshot = &self.snapshot;
        if snapshot.format_version != 1
            || snapshot.order_revision <= 0
            || snapshot.created_at <= 0
            || snapshot.sources.len() > MAX_INPUT_SOURCES
            || snapshot.sources.len() != self.origins.len()
            || snapshot.id.is_empty()
            || uuid::Uuid::parse_str(&snapshot.id).is_err()
            || !workspace_transfer::valid_sha256(&snapshot.request_sha256)
            || self.sha256 != canonical_hash(snapshot)
        {
            return Err(Error::Conflict);
        }
        valid_text(&snapshot.order_id, 128)?;
        valid_text(&snapshot.project_id, 128)?;
        workspace_transfer::validate(&snapshot.manifest)?;
        let mut source_ids = BTreeSet::new();
        let mut native_ids = BTreeSet::new();
        for (i, (source, origin)) in snapshot.sources.iter().zip(&self.origins).enumerate() {
            if source.alias != source_alias(i)
                || source.name != origin.name
                || origin.project_id != snapshot.project_id
                || origin
                    .order_id
                    .as_ref()
                    .is_some_and(|order| order != &snapshot.order_id)
                || !source_ids.insert(&origin.id)
                || !native_ids.insert(&origin.resource_id)
                || !workspace_transfer::valid_sha256(&origin.resource_id)
                || !workspace_transfer::valid_sha256(&origin.revision)
                || !snapshot.manifest.directories.contains(&source.alias)
            {
                return Err(Error::Forbidden);
            }
            valid_text(&origin.id, 256)?;
            valid_text(&origin.name, 200)?;
        }
        let aliases: BTreeSet<_> = snapshot.sources.iter().map(|s| s.alias.as_str()).collect();
        for path in snapshot
            .manifest
            .directories
            .iter()
            .map(String::as_str)
            .chain(snapshot.manifest.files.iter().map(|f| f.path.as_str()))
        {
            if !aliases.contains(path.split('/').next().unwrap_or_default()) {
                return Err(Error::Forbidden);
            }
        }
        Ok(())
    }

    pub fn validate_request(&self, request: &PrepareExecutionInput) -> Result<()> {
        validate_request(request)?;
        self.validate()?;
        if self.snapshot.request_sha256 != request_hash(&self.snapshot.order_id, request)
            || self.snapshot.order_revision != request.base_revision
            || self
                .origins
                .iter()
                .map(|g| &g.id)
                .ne(request.folder_ids.iter())
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
