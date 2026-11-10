//! Portable content identities. These records contain no native paths or authority.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTransferChunk {
    pub offset: u64,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTransferFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub chunks: Vec<WorkspaceTransferChunk>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTransferManifest {
    pub format_version: u32,
    pub directories: Vec<String>,
    pub files: Vec<WorkspaceTransferFile>,
    pub total_bytes: u64,
    pub sha256: String,
}

/// A chunk is always read through an authenticated source order/run, never by a
/// global content hash alone. The expected hash is an integrity check, not a grant.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadWorkspaceTransferChunk {
    pub path: String,
    pub offset: u64,
    pub expected_sha256: String,
}
