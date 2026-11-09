//! Explicit private peer inventory; runtime data never supplies transport targets.
use std::{collections::HashSet, net::SocketAddr, path::PathBuf};

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterConfig {
    pub group_id: Uuid,
    pub node_id: Uuid,
    pub listen_addr: SocketAddr,
    pub certificate_file: PathBuf,
    pub private_key_file: PathBuf,
    pub ca_certificate_file: PathBuf,
    pub peers: Vec<PeerConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
    pub node_id: Uuid,
    pub address: SocketAddr,
    pub server_name: String,
    pub certificate_sha256: String,
}

impl ClusterConfig {
    pub fn from_env() -> Result<Option<Self>> {
        let Some(path) = std::env::var_os("COWORK_API_CLUSTER_CONFIG_FILE") else {
            return Ok(None);
        };
        let bytes = read_bounded(&PathBuf::from(path), false)?;
        let config: Self =
            serde_json::from_slice(&bytes).context("invalid private API cluster configuration")?;
        config.validate()?;
        Ok(Some(config))
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            !self.group_id.is_nil() && !self.node_id.is_nil(),
            "cluster group/node IDs must be non-nil"
        );
        ensure!(
            self.peers.len() >= 2 && self.peers.len() <= 8,
            "cluster requires an explicit inventory of 2 to 8 peers including self"
        );
        ensure!(
            private_address(self.listen_addr),
            "private API listener must use an explicit private or loopback IP and port"
        );
        let mut nodes = HashSet::new();
        let mut pins = HashSet::new();
        let mut addresses = HashSet::new();
        for peer in &self.peers {
            ensure!(
                !peer.node_id.is_nil() && nodes.insert(peer.node_id),
                "duplicate or nil private peer node"
            );
            ensure!(
                private_address(peer.address) && addresses.insert(peer.address),
                "duplicate or non-private peer address"
            );
            ensure!(
                peer.server_name.len() <= 253
                    && !peer.server_name.is_empty()
                    && peer
                        .server_name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')),
                "private TLS peer requires a DNS server name"
            );
            ensure!(
                peer.certificate_sha256.len() == 64
                    && peer
                        .certificate_sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && pins.insert(peer.certificate_sha256.clone()),
                "peer certificate pins must be distinct SHA256 hashes"
            );
        }
        let own = self.peer(self.node_id)?;
        ensure!(
            own.address == self.listen_addr,
            "self peer address must match the private listener"
        );
        Ok(())
    }

    pub(super) fn peer(&self, node_id: Uuid) -> Result<&PeerConfig> {
        self.peers
            .iter()
            .find(|peer| peer.node_id == node_id)
            .context("node is not in configured private peer inventory")
    }

    pub(super) fn peer_for_certificate(&self, fingerprint: &str) -> Result<&PeerConfig> {
        self.peers
            .iter()
            .find(|peer| peer.certificate_sha256 == fingerprint)
            .context("TLS certificate does not identify an allowed cluster peer")
    }
}

fn private_address(address: SocketAddr) -> bool {
    address.port() != 0
        && match address.ip() {
            std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
            std::net::IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
        }
}

pub(super) fn read_bounded(path: &std::path::Path, private: bool) -> Result<Vec<u8>> {
    use std::io::Read;
    let metadata =
        std::fs::symlink_metadata(path).context("private API TLS/config file unavailable")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 1024 * 1024,
        "private API TLS/config file must be a bounded regular file"
    );
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.mode() & 0o077 == 0,
            "private API TLS key must be owner-only"
        );
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= 1024 * 1024,
        "private API TLS/config file exceeds bounds"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> ClusterConfig {
        let node_id = Uuid::new_v4();
        ClusterConfig {
            group_id: Uuid::new_v4(),
            node_id,
            listen_addr: "127.0.0.1:9001".parse().unwrap(),
            certificate_file: "unused".into(),
            private_key_file: "unused".into(),
            ca_certificate_file: "unused".into(),
            peers: vec![
                PeerConfig {
                    node_id,
                    address: "127.0.0.1:9001".parse().unwrap(),
                    server_name: "one.internal".into(),
                    certificate_sha256: "1".repeat(64),
                },
                PeerConfig {
                    node_id: Uuid::new_v4(),
                    address: "127.0.0.1:9002".parse().unwrap(),
                    server_name: "two.internal".into(),
                    certificate_sha256: "2".repeat(64),
                },
            ],
        }
    }
    #[test]
    fn peer_targets_and_identity_are_only_allowlisted() {
        let valid = config();
        assert!(valid.validate().is_ok());
        assert!(valid.peer(Uuid::new_v4()).is_err());
        assert!(valid.peer_for_certificate(&"3".repeat(64)).is_err());
        assert_eq!(
            valid.peer_for_certificate(&"1".repeat(64)).unwrap().node_id,
            valid.node_id
        );
        assert_eq!(
            valid.peer_for_certificate(&"2".repeat(64)).unwrap().node_id,
            valid.peers[1].node_id
        );
        let mut bad = valid.clone();
        bad.peers[1].address = "8.8.8.8:9002".parse().unwrap();
        assert!(bad.validate().is_err());
        let mut bad = valid.clone();
        bad.peers[1].certificate_sha256 = bad.peers[0].certificate_sha256.clone();
        assert!(bad.validate().is_err());
        let mut bad = valid;
        bad.peers[1].server_name = "https://payload.invalid/path".into();
        assert!(bad.validate().is_err());
    }
}
