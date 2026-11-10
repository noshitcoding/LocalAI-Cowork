//! Optional native TLS. Application request authentication sees original bytes.
use anyhow::{ensure, Context, Result};
use std::{io::Read, net::SocketAddr, path::Path, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer},
        RootCertStore,
    },
    TlsAcceptor,
};

const MAX_PEM: usize = 1024 * 1024;

pub(crate) fn configured() -> Result<Option<TlsAcceptor>> {
    let cert = std::env::var_os("COWORK_RUNNER_TLS_CERTIFICATE_FILE");
    let key = std::env::var_os("COWORK_RUNNER_TLS_PRIVATE_KEY_FILE");
    let ca = std::env::var_os("COWORK_RUNNER_TLS_CLIENT_CA_FILE");
    let (Some(cert), Some(key)) = (cert, key) else {
        ensure!(
            std::env::var_os("COWORK_RUNNER_TLS_CERTIFICATE_FILE").is_none()
                && std::env::var_os("COWORK_RUNNER_TLS_PRIVATE_KEY_FILE").is_none()
                && ca.is_none(),
            "runner TLS requires both certificate and private key"
        );
        return Ok(None);
    };
    let cert = read_bounded(Path::new(&cert), false)?;
    let key = read_bounded(Path::new(&key), true)?;
    let ca = ca
        .map(|path| read_bounded(Path::new(&path), false))
        .transpose()?;
    acceptor(&cert, &key, ca.as_deref()).map(Some)
}

fn read_bounded(path: &Path, private: bool) -> Result<Vec<u8>> {
    ensure!(
        path.is_absolute(),
        "runner TLS file must use an absolute path"
    );
    let metadata = std::fs::symlink_metadata(path).context("runner TLS file unavailable")?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= MAX_PEM as u64,
        "runner TLS file must be bounded and regular"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.nlink() == 1,
            "runner TLS file must not have hardlink aliases"
        );
        if private {
            ensure!(
                metadata.mode() & 0o077 == 0,
                "runner TLS private key must be owner-only"
            );
        }
    }
    #[cfg(not(unix))]
    let _ = private;
    let file = std::fs::File::open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file() && opened.len() <= MAX_PEM as u64,
        "runner TLS file changed during open"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.dev() == opened.dev()
                && metadata.ino() == opened.ino()
                && opened.nlink() == 1
                && (!private || opened.mode() & 0o077 == 0),
            "runner TLS file identity or permissions changed during open"
        );
    }
    let mut bytes = Vec::new();
    file.take(MAX_PEM as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_PEM,
        "runner TLS file exceeds bounds"
    );
    if !private {
        ensure!(
            !bytes.windows(11).any(|bytes| bytes == b"PRIVATE KEY"),
            "runner public TLS file contains a private key"
        );
    }
    Ok(bytes)
}

fn acceptor(cert: &[u8], key: &[u8], ca: Option<&[u8]>) -> Result<TlsAcceptor> {
    let certificates =
        CertificateDer::pem_slice_iter(cert).collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        !certificates.is_empty(),
        "runner TLS certificate chain is empty"
    );
    let keys = PrivateKeyDer::pem_slice_iter(key).collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        keys.len() == 1,
        "runner TLS requires exactly one private key"
    );
    let key = keys.into_iter().next().expect("one key checked");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?;
    let mut config = if let Some(ca) = ca {
        let mut roots = RootCertStore::empty();
        for certificate in CertificateDer::pem_slice_iter(ca) {
            roots.add(certificate?)?;
        }
        ensure!(!roots.is_empty(), "runner client trust roots are empty");
        let verifier =
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()?;
        builder
            .with_client_cert_verifier(verifier)
            .with_single_cert(certificates, key)?
    } else {
        builder
            .with_no_client_auth()
            .with_single_cert(certificates, key)?
    };
    // Axum's HTTP/1 WebSocket upgrade also works over these original TLS bytes.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

type Accepted = Result<(tokio_rustls::server::TlsStream<TcpStream>, SocketAddr)>;
pub(crate) struct Listener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    pending: JoinSet<Accepted>,
}
impl Listener {
    pub(crate) fn new(tcp: TcpListener, acceptor: TlsAcceptor) -> Self {
        Self {
            tcp,
            acceptor,
            pending: JoinSet::new(),
        }
    }
}
impl axum::serve::Listener for Listener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = SocketAddr;
    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.tcp.local_addr()
    }
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                accepted=self.tcp.accept(),if self.pending.len()<64=>match accepted {
                    Ok((tcp,address))=>{
                        let acceptor=self.acceptor.clone();
                        self.pending.spawn(async move {
                            let stream=tokio::time::timeout(Duration::from_secs(5),acceptor.accept(tcp)).await.context("runner TLS handshake deadline exceeded")??;
                            Ok((stream,address))
                        });
                    },
                    Err(error)=>{tracing::warn!(?error,"runner TCP accept failed");tokio::time::sleep(Duration::from_millis(100)).await;},
                },
                completed=self.pending.join_next(),if !self.pending.is_empty()=>match completed {
                    Some(Ok(Ok(accepted)))=>return accepted,
                    failure=>tracing::debug!(?failure,"runner rejected TLS handshake"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_or_ambiguous_tls_material_is_rejected() {
        assert!(acceptor(b"not PEM", b"not PEM", None).is_err());
        assert!(acceptor(
            b"-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----",
            b"",
            None
        )
        .is_err());
    }
    #[test]
    fn tls_reads_are_bounded_regular_and_public_files_cannot_contain_keys() {
        let root =
            std::env::temp_dir().join(format!("cowork-runner-tls-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("fixture.pem");
        std::fs::write(&file, b"fixture public material").unwrap();
        assert!(read_bounded(&file, false).is_ok());
        assert!(read_bounded(Path::new("relative.pem"), false).is_err());
        std::fs::write(&file, b"-----BEGIN PRIVATE KEY-----").unwrap();
        assert!(read_bounded(&file, false).is_err());
        std::fs::write(&file, vec![0; MAX_PEM + 1]).unwrap();
        assert!(read_bounded(&file, true).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            std::fs::write(&file, b"private fixture material").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_bounded(&file, true).is_err());
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(read_bounded(&file, true).is_ok());
            let link = root.join("alias.pem");
            symlink(&file, &link).unwrap();
            assert!(read_bounded(&link, true).is_err());
            let hard = root.join("hard.pem");
            std::fs::hard_link(&file, &hard).unwrap();
            assert!(read_bounded(&file, true).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
