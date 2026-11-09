//! mTLS identity plus fixed-size private RFB byte-stream frames.
use anyhow::{ensure, Context, Result};
use openssl::{pkey::PKey, x509::X509};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{pin::Pin, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
        RootCertStore,
    },
    TlsAcceptor, TlsConnector,
};
use uuid::Uuid;

use super::{
    config::{read_bounded, ClusterConfig},
    ApiCluster,
};

const MAX_INITIAL: usize = 16 * 1024;
const FRAME_BYTES: usize = 256 * 1024;
pub(crate) trait PeerIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> PeerIo for T {}
pub(crate) type PeerSocket = Pin<Box<dyn PeerIo>>;

#[derive(Clone)]
pub(super) struct Tls {
    pub acceptor: TlsAcceptor,
    pub connector: TlsConnector,
    pub own_sha256: Vec<u8>,
}

impl Tls {
    pub fn new(config: &ClusterConfig) -> Result<Self> {
        let ca = read_bounded(&config.ca_certificate_file, false)?;
        let cert = read_bounded(&config.certificate_file, false)?;
        ensure!(
            !ca.windows(11).any(|bytes| bytes == b"PRIVATE KEY")
                && !cert.windows(11).any(|bytes| bytes == b"PRIVATE KEY"),
            "public TLS material contains a private key"
        );
        let certificates = X509::stack_from_pem(&cert)?;
        ensure!(
            !certificates.is_empty(),
            "API TLS certificate chain is empty"
        );
        let own_sha256 = Sha256::digest(certificates[0].to_der()?).to_vec();
        ensure!(
            hex::encode(&own_sha256) == config.peer(config.node_id)?.certificate_sha256,
            "API certificate differs from configured node identity"
        );
        let chain = certificates
            .into_iter()
            .map(|cert| cert.to_der().map(CertificateDer::from))
            .collect::<Result<Vec<_>, _>>()?;
        let key = read_bounded(&config.private_key_file, true)?;
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            PKey::private_key_from_pem(&key)?.private_key_to_pkcs8()?,
        ));
        let mut roots = RootCertStore::empty();
        for cert in X509::stack_from_pem(&ca)? {
            roots.add(CertificateDer::from(cert.to_der()?))?;
        }
        ensure!(!roots.is_empty(), "API private trust roots are empty");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots.clone()),
            provider.clone(),
        )
        .build()?;
        let server = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()?
            .with_client_cert_verifier(verifier)
            .with_single_cert(chain.clone(), key.clone_key())?;
        let client = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_client_auth_cert(chain, key)?;
        Ok(Self {
            acceptor: TlsAcceptor::from(Arc::new(server)),
            connector: TlsConnector::from(Arc::new(client)),
            own_sha256,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    User,
    ExecutorCallback,
    DesktopStop,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub protocol: u16,
    pub group_id: Uuid,
    pub source_node_id: Uuid,
    pub source_process_epoch: Uuid,
    pub target_node_id: Uuid,
    pub target_process_epoch: Uuid,
    pub stream_id: Uuid,
    pub role: Role,
    pub executor_id: Option<Uuid>,
    pub credential_id: Option<Uuid>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Ack {
    protocol: u16,
    accepted: bool,
}

pub(super) fn identity(
    config: &ClusterConfig,
    actual_sha: &str,
    request: &Request,
) -> Result<Uuid> {
    let peer = config.peer_for_certificate(actual_sha)?;
    ensure!(
        request.protocol == 1
            && request.group_id == config.group_id
            && request.source_node_id == peer.node_id
            && request.target_node_id == config.node_id,
        "private TLS peer identity/group/protocol mismatch"
    );
    Ok(peer.node_id)
}

pub(super) async fn open(cluster: &ApiCluster, request: &Request) -> Result<PeerSocket> {
    cluster.ensure_active().await?;
    let peer = cluster.inner.config.peer(request.target_node_id)?;
    let socket = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        TcpStream::connect(peer.address),
    )
    .await??;
    let name = ServerName::try_from(peer.server_name.clone())?;
    let tls = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        cluster.inner.tls.connector.connect(name, socket),
    )
    .await??;
    let certificate = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .context("private API peer certificate missing")?;
    ensure!(
        hex::encode(Sha256::digest(certificate.as_ref())) == peer.certificate_sha256,
        "private API server certificate pin mismatch"
    );
    let mut socket: PeerSocket = Box::pin(tls);
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        write_initial(&mut socket, request),
    )
    .await??;
    let ack: Ack =
        tokio::time::timeout(std::time::Duration::from_secs(5), read_initial(&mut socket))
            .await??;
    ensure!(
        ack.protocol == 1 && ack.accepted,
        "private API rejected one-use desktop ticket"
    );
    Ok(socket)
}

pub(super) async fn read_initial<T: serde::de::DeserializeOwned>(
    socket: &mut PeerSocket,
) -> Result<T> {
    let length = socket.read_u32().await? as usize;
    ensure!(
        length > 0 && length <= MAX_INITIAL,
        "private initial frame exceeds bounds"
    );
    let mut bytes = vec![0; length];
    socket.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
pub(super) async fn write_initial<T: Serialize>(socket: &mut PeerSocket, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= MAX_INITIAL,
        "private initial frame exceeds bounds"
    );
    socket.write_u32(bytes.len() as u32).await?;
    socket.write_all(&bytes).await?;
    socket.flush().await?;
    Ok(())
}
pub(super) async fn acknowledge(socket: &mut PeerSocket) -> Result<()> {
    write_initial(
        socket,
        &Ack {
            protocol: 1,
            accepted: true,
        },
    )
    .await
}

/// Retains partially consumed frames when a competing select branch wins.
/// Only cancellation-safe reads await; their offsets live beyond each future.
#[derive(Default)]
pub(crate) struct FrameReader {
    header: [u8; 4],
    header_read: usize,
    body: Vec<u8>,
    body_read: usize,
    ended: bool,
    failed: bool,
}

impl FrameReader {
    pub(crate) async fn read_frame(
        &mut self,
        socket: &mut (impl AsyncRead + Unpin),
    ) -> Result<Option<Vec<u8>>> {
        ensure!(!self.failed, "private RFB frame reader has failed");
        if self.ended {
            return Ok(None);
        }
        while self.header_read < self.header.len() {
            match socket.read(&mut self.header[self.header_read..]).await {
                Ok(0) => {
                    self.failed = true;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "private RFB frame header truncated",
                    )
                    .into());
                }
                Ok(count) => self.header_read += count,
                Err(error) => {
                    self.failed = true;
                    return Err(error.into());
                }
            }
        }
        if self.body.is_empty() {
            let length = u32::from_be_bytes(self.header) as usize;
            self.failed = length > FRAME_BYTES;
            ensure!(!self.failed, "private RFB frame exceeds bounds");
            if length == 0 {
                self.ended = true;
                return Ok(None);
            }
            self.body.resize(length, 0);
        }
        while self.body_read < self.body.len() {
            match socket.read(&mut self.body[self.body_read..]).await {
                Ok(0) => {
                    self.failed = true;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "private RFB frame body truncated",
                    )
                    .into());
                }
                Ok(count) => self.body_read += count,
                Err(error) => {
                    self.failed = true;
                    return Err(error.into());
                }
            }
        }
        self.header_read = 0;
        self.body_read = 0;
        Ok(Some(std::mem::take(&mut self.body)))
    }
}
pub(crate) async fn write_frames(
    socket: &mut (impl AsyncWrite + Unpin),
    bytes: &[u8],
) -> Result<()> {
    for chunk in bytes.chunks(FRAME_BYTES) {
        socket.write_u32(chunk.len() as u32).await?;
        socket.write_all(chunk).await?;
    }
    socket.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tls_peer_claim_cannot_override_certificate_group_target_or_protocol() {
        use super::super::config::PeerConfig;
        let self_id = Uuid::new_v4();
        let other = Uuid::new_v4();
        let config = ClusterConfig {
            group_id: Uuid::new_v4(),
            node_id: self_id,
            listen_addr: "127.0.0.1:19001".parse().unwrap(),
            certificate_file: "unused".into(),
            private_key_file: "unused".into(),
            ca_certificate_file: "unused".into(),
            peers: vec![
                PeerConfig {
                    node_id: self_id,
                    address: "127.0.0.1:19001".parse().unwrap(),
                    server_name: "one.internal".into(),
                    certificate_sha256: "1".repeat(64),
                },
                PeerConfig {
                    node_id: other,
                    address: "127.0.0.1:19002".parse().unwrap(),
                    server_name: "two.internal".into(),
                    certificate_sha256: "2".repeat(64),
                },
            ],
        };
        let mut request = Request {
            protocol: 1,
            group_id: config.group_id,
            source_node_id: other,
            source_process_epoch: Uuid::new_v4(),
            target_node_id: self_id,
            target_process_epoch: Uuid::new_v4(),
            stream_id: Uuid::new_v4(),
            role: Role::User,
            executor_id: None,
            credential_id: None,
        };
        assert_eq!(identity(&config, &"2".repeat(64), &request).unwrap(), other);
        assert!(identity(&config, &"3".repeat(64), &request).is_err());
        request.source_node_id = self_id;
        assert!(identity(&config, &"2".repeat(64), &request).is_err());
        request.source_node_id = other;
        request.group_id = Uuid::new_v4();
        assert!(identity(&config, &"2".repeat(64), &request).is_err());
        request.group_id = config.group_id;
        request.target_node_id = other;
        assert!(identity(&config, &"2".repeat(64), &request).is_err());
        request.target_node_id = self_id;
        request.protocol = 2;
        assert!(identity(&config, &"2".repeat(64), &request).is_err());
        let mut json = serde_json::to_value(request).unwrap();
        json["callback_url"] = serde_json::json!("https://payload.invalid");
        assert!(serde_json::from_value::<Request>(json).is_err());
    }

    fn certificate(
        name: &str,
        issuer: Option<(&X509, &PKey<openssl::pkey::Private>)>,
    ) -> (X509, PKey<openssl::pkey::Private>) {
        use openssl::{
            asn1::Asn1Time,
            bn::BigNum,
            hash::MessageDigest,
            rsa::Rsa,
            x509::{
                extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName},
                X509NameBuilder,
            },
        };
        let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let mut builder = X509::builder().unwrap();
        builder.set_version(2).unwrap();
        let serial = BigNum::from_u32(if issuer.is_none() { 1 } else { 2 })
            .unwrap()
            .to_asn1_integer()
            .unwrap();
        builder.set_serial_number(&serial).unwrap();
        let mut dn = X509NameBuilder::new().unwrap();
        dn.append_entry_by_text("CN", name).unwrap();
        let dn = dn.build();
        builder.set_subject_name(&dn).unwrap();
        builder
            .set_issuer_name(issuer.map(|i| i.0.subject_name()).unwrap_or(&dn))
            .unwrap();
        builder.set_pubkey(&key).unwrap();
        builder
            .set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        builder
            .set_not_after(&Asn1Time::days_from_now(2).unwrap())
            .unwrap();
        if issuer.is_none() {
            builder
                .append_extension(BasicConstraints::new().critical().ca().build().unwrap())
                .unwrap();
            builder
                .append_extension(
                    KeyUsage::new()
                        .critical()
                        .key_cert_sign()
                        .crl_sign()
                        .build()
                        .unwrap(),
                )
                .unwrap();
        } else {
            builder
                .append_extension(BasicConstraints::new().critical().build().unwrap())
                .unwrap();
            builder
                .append_extension(
                    KeyUsage::new()
                        .critical()
                        .digital_signature()
                        .key_encipherment()
                        .build()
                        .unwrap(),
                )
                .unwrap();
            builder
                .append_extension(
                    ExtendedKeyUsage::new()
                        .server_auth()
                        .client_auth()
                        .build()
                        .unwrap(),
                )
                .unwrap();
            let san = SubjectAlternativeName::new()
                .dns(name)
                .build(&builder.x509v3_context(issuer.map(|i| i.0.as_ref()), None))
                .unwrap();
            builder.append_extension(san).unwrap();
        }
        builder
            .sign(issuer.map(|i| i.1).unwrap_or(&key), MessageDigest::sha256())
            .unwrap();
        (builder.build(), key)
    }

    #[tokio::test]
    async fn private_tls_requires_client_certificate_and_checks_server_hostname() {
        use super::super::config::PeerConfig;
        let (ca, ca_key) = certificate("fixture CA", None);
        let (a, a_key) = certificate("one.internal", Some((&ca, &ca_key)));
        let (b, b_key) = certificate("two.internal", Some((&ca, &ca_key)));
        let directory = std::env::temp_dir().join(format!("cowork-cluster-tls-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        for (name, body) in [
            ("ca.pem", ca.to_pem().unwrap()),
            ("a.pem", a.to_pem().unwrap()),
            ("b.pem", b.to_pem().unwrap()),
            ("a.key", a_key.private_key_to_pem_pkcs8().unwrap()),
            ("b.key", b_key.private_key_to_pem_pkcs8().unwrap()),
        ] {
            let path = directory.join(name);
            std::fs::write(&path, body).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();
        let peers = vec![
            PeerConfig {
                node_id: a_id,
                address: "127.0.0.1:19001".parse().unwrap(),
                server_name: "one.internal".into(),
                certificate_sha256: hex::encode(Sha256::digest(a.to_der().unwrap())),
            },
            PeerConfig {
                node_id: b_id,
                address: "127.0.0.1:19002".parse().unwrap(),
                server_name: "two.internal".into(),
                certificate_sha256: hex::encode(Sha256::digest(b.to_der().unwrap())),
            },
        ];
        let a_config = ClusterConfig {
            group_id: Uuid::new_v4(),
            node_id: a_id,
            listen_addr: peers[0].address,
            certificate_file: directory.join("a.pem"),
            private_key_file: directory.join("a.key"),
            ca_certificate_file: directory.join("ca.pem"),
            peers,
        };
        let mut b_config = a_config.clone();
        b_config.node_id = b_id;
        b_config.listen_addr = b_config.peers[1].address;
        b_config.certificate_file = directory.join("b.pem");
        b_config.private_key_file = directory.join("b.key");
        let a_tls = Tls::new(&a_config).unwrap();
        let b_tls = Tls::new(&b_config).unwrap();
        let (client, server) = tokio::io::duplex(16384);
        let (server_result, client_result) = tokio::join!(
            b_tls.acceptor.accept(server),
            a_tls
                .connector
                .connect(ServerName::try_from("two.internal").unwrap(), client)
        );
        assert!(server_result.is_ok());
        let client = client_result.unwrap();
        let leaf = &client.get_ref().1.peer_certificates().unwrap()[0];
        assert_eq!(
            hex::encode(Sha256::digest(leaf.as_ref())),
            a_config.peers[1].certificate_sha256
        );
        let (client, server) = tokio::io::duplex(16384);
        let (_, wrong_name) = tokio::join!(
            b_tls.acceptor.accept(server),
            a_tls
                .connector
                .connect(ServerName::try_from("wrong.internal").unwrap(), client)
        );
        assert!(wrong_name.is_err());
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(ca.to_der().unwrap()))
            .unwrap();
        let anonymous = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let anonymous = TlsConnector::from(Arc::new(anonymous));
        let (client, server) = tokio::io::duplex(16384);
        let (rejected, _) = tokio::join!(
            b_tls.acceptor.accept(server),
            anonymous.connect(ServerName::try_from("two.internal").unwrap(), client)
        );
        assert!(rejected.is_err());
    }

    #[tokio::test]
    async fn private_frames_preserve_rfb_bytes_and_reject_oversized_allocation() {
        let (mut writer, mut reader) = tokio::io::duplex(FRAME_BYTES * 2);
        let expected = vec![42; FRAME_BYTES + 7];
        let body = expected.clone();
        let send = tokio::spawn(async move {
            write_frames(&mut writer, &body).await.unwrap();
        });
        let mut frames = FrameReader::default();
        let mut actual = frames.read_frame(&mut reader).await.unwrap().unwrap();
        actual.extend(frames.read_frame(&mut reader).await.unwrap().unwrap());
        assert_eq!(actual, expected);
        send.await.unwrap();
        let (mut writer, mut reader) = tokio::io::duplex(4);
        writer.write_u32(FRAME_BYTES as u32 + 1).await.unwrap();
        assert!(FrameReader::default()
            .read_frame(&mut reader)
            .await
            .is_err());
    }
}

#[cfg(test)]
mod frame_regressions {
    use super::*;
    use tokio::io::DuplexStream;

    async fn competing_frame_cancels_partial_read(
        frames: &mut FrameReader,
        incoming: &mut DuplexStream,
        reverse_frames: &mut FrameReader,
        reverse: &mut DuplexStream,
    ) {
        tokio::select! {
            biased;
            result = frames.read_frame(incoming) => {
                panic!("incomplete frame unexpectedly finished: {result:?}");
            }
            result = reverse_frames.read_frame(reverse) => {
                assert_eq!(result.unwrap().unwrap(), b"opposite");
            }
        }
    }

    #[tokio::test]
    async fn private_fragmented_header_survives_competing_direction() {
        for split in 1..4 {
            let (mut writer, mut reader) = tokio::io::duplex(64);
            let (mut reverse_writer, mut reverse_reader) = tokio::io::duplex(64);
            let mut frames = FrameReader::default();
            let mut reverse_frames = FrameReader::default();
            let header = 6_u32.to_be_bytes();
            writer.write_all(&header[..split]).await.unwrap();
            write_frames(&mut reverse_writer, b"opposite")
                .await
                .unwrap();
            competing_frame_cancels_partial_read(
                &mut frames,
                &mut reader,
                &mut reverse_frames,
                &mut reverse_reader,
            )
            .await;
            writer.write_all(&header[split..]).await.unwrap();
            writer.write_all(b"RFBabc").await.unwrap();
            write_frames(&mut writer, b"next").await.unwrap();
            writer.write_u32(0).await.unwrap();
            drop(writer);
            assert_eq!(
                frames.read_frame(&mut reader).await.unwrap().unwrap(),
                b"RFBabc"
            );
            assert_eq!(
                frames.read_frame(&mut reader).await.unwrap().unwrap(),
                b"next"
            );
            assert!(frames.read_frame(&mut reader).await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn private_fragmented_body_survives_repeated_select_cancellation() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        let (mut reverse_writer, mut reverse_reader) = tokio::io::duplex(64);
        let mut frames = FrameReader::default();
        let mut reverse_frames = FrameReader::default();
        let payload = b"RFBbody!";
        let mut encoded = (payload.len() as u32).to_be_bytes().to_vec();
        encoded.extend_from_slice(payload);
        for byte in &encoded[..encoded.len() - 1] {
            writer.write_all(&[*byte]).await.unwrap();
            write_frames(&mut reverse_writer, b"opposite")
                .await
                .unwrap();
            competing_frame_cancels_partial_read(
                &mut frames,
                &mut reader,
                &mut reverse_frames,
                &mut reverse_reader,
            )
            .await;
        }
        writer
            .write_all(&encoded[encoded.len() - 1..])
            .await
            .unwrap();
        write_frames(&mut writer, b"following").await.unwrap();
        drop(writer);
        assert_eq!(
            frames.read_frame(&mut reader).await.unwrap().unwrap(),
            payload
        );
        assert_eq!(
            frames.read_frame(&mut reader).await.unwrap().unwrap(),
            b"following"
        );
    }

    #[tokio::test]
    async fn private_frame_eof_and_truncation_remain_terminal() {
        let encoded = [0, 0, 0, 4, 10, 11, 12, 13];
        for cut in 0..encoded.len() {
            let (mut writer, mut reader) = tokio::io::duplex(16);
            writer.write_all(&encoded[..cut]).await.unwrap();
            drop(writer);
            let mut frames = FrameReader::default();
            let error = frames.read_frame(&mut reader).await.unwrap_err();
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::UnexpectedEof
            );
            assert!(frames.read_frame(&mut reader).await.is_err());
        }
    }

    #[tokio::test]
    async fn private_frame_zero_marker_is_terminal() {
        let (mut writer, mut reader) = tokio::io::duplex(32);
        writer.write_u32(0).await.unwrap();
        write_frames(&mut writer, b"after end").await.unwrap();
        let mut frames = FrameReader::default();
        assert!(frames.read_frame(&mut reader).await.unwrap().is_none());
        assert!(frames.read_frame(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn private_frame_oversized_header_fails_before_body_allocation() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        writer.write_u32(FRAME_BYTES as u32 + 1).await.unwrap();
        let mut frames = FrameReader::default();
        assert!(frames.read_frame(&mut reader).await.is_err());
        assert_eq!(frames.body.capacity(), 0);
        assert!(frames.read_frame(&mut reader).await.is_err());
    }

    #[tokio::test]
    async fn private_frame_reader_preserves_chunking_and_backpressure() {
        let (mut writer, mut reader) = tokio::io::duplex(4);
        let (done, mut completion) = tokio::sync::oneshot::channel();
        let expected = vec![42; FRAME_BYTES + 7];
        let body = expected.clone();
        let send = tokio::spawn(async move {
            write_frames(&mut writer, &body).await.unwrap();
            done.send(()).unwrap();
        });
        let mut frames = FrameReader::default();
        let mut actual = frames.read_frame(&mut reader).await.unwrap().unwrap();
        assert_eq!(actual.len(), FRAME_BYTES);
        assert_eq!(
            completion.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        );
        actual.extend(frames.read_frame(&mut reader).await.unwrap().unwrap());
        assert_eq!(actual, expected);
        completion.await.unwrap();
        send.await.unwrap();
    }
}
