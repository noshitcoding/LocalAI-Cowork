//! An owned verified-TLS transport bounds BER before ldap3's unbounded decoder.
//! The private connected loopback pair is never a published LDAP service.
use super::*;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_rustls::{
    rustls::{
        pki_types::{CertificateDer, ServerName},
        ClientConfig, RootCertStore,
    },
    TlsConnector,
};
pub(super) struct Connection {
    pub ldap: ldap3::Ldap,
    _tasks: OwnedTasks,
}
struct OwnedTasks(Vec<JoinHandle<()>>);
impl Drop for OwnedTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}
pub(super) async fn connect(connector: &catalog::Connector) -> Result<Connection, ApiError> {
    let l = connector.ldap.as_ref().ok_or_else(unavailable)?;
    let url = reqwest::Url::parse(&l.url).map_err(|_| unavailable())?;
    let host = url.host_str().ok_or_else(unavailable)?.to_owned();
    let mut roots = RootCertStore::empty();
    if let Some(certs) = &connector.ca_certificates {
        for cert in certs {
            roots
                .add(CertificateDer::from(cert.clone()))
                .map_err(|_| unavailable())?;
        }
    } else {
        let certs = rustls_native_certs::load_native_certs();
        for cert in certs.certs {
            let _ = roots.add(cert);
        }
    }
    if roots.is_empty() {
        return Err(unavailable());
    }
    let config = ClientConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| unavailable())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let remote = TcpStream::connect((host.as_str(), url.port().unwrap_or(636)))
        .await
        .map_err(|_| unavailable())?;
    let tls = TlsConnector::from(Arc::new(config))
        .connect(
            ServerName::try_from(host).map_err(|_| unavailable())?,
            remote,
        )
        .await
        .map_err(|_| unavailable())?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| unavailable())?;
    let client = TcpStream::connect(listener.local_addr().map_err(|_| unavailable())?)
        .await
        .map_err(|_| unavailable())?;
    let (local, peer) = listener.accept().await.map_err(|_| unavailable())?;
    if peer != client.local_addr().map_err(|_| unavailable())? {
        return Err(unavailable());
    }
    drop(listener);
    let (mut lr, mut lw) = local.into_split();
    let (mut rr, mut rw) = tokio::io::split(tls);
    let relay = tokio::spawn(async move {
        // Both directions have frame, total-byte, message and elapsed limits.
        let _ = tokio::time::timeout(Duration::from_secs(30), async {
            tokio::select! {_ = frames(&mut lr,&mut rw)=>{},_ = frames(&mut rr,&mut lw)=>{}}
        })
        .await;
    });
    // Cancellation while constructing ldap3 also drops/aborts this exact relay.
    let mut tasks = OwnedTasks(vec![relay]);
    let settings = ldap3::LdapConnSettings::new()
        .set_conn_timeout(Duration::from_secs(5))
        .set_std_stream(ldap3::StdStream::Tcp(
            client.into_std().map_err(|_| unavailable())?,
        ));
    let result = ldap3::LdapConnAsync::with_settings(settings, "ldap://127.0.0.1:1").await;
    let (conn, ldap) = match result {
        Ok(v) => v,
        Err(_) => {
            return Err(unavailable());
        }
    };
    let drive = tokio::spawn(async move {
        let _ = conn.drive().await;
    });
    tasks.0.push(drive);
    Ok(Connection {
        ldap,
        _tasks: tasks,
    })
}
async fn frames<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
) -> std::io::Result<()> {
    let mut total = 0usize;
    for _ in 0..1002 {
        let mut prefix = [0u8; 2];
        reader.read_exact(&mut prefix).await?;
        if prefix[0] != 0x30 {
            return Err(bad());
        }
        let mut frame = prefix.to_vec();
        let n = if prefix[1] & 0x80 == 0 {
            prefix[1] as usize
        } else {
            let size = (prefix[1] & 0x7f) as usize;
            if size == 0 || size > 4 {
                return Err(bad());
            }
            let mut length = vec![0; size];
            reader.read_exact(&mut length).await?;
            if length[0] == 0 {
                return Err(bad());
            }
            let n = length.iter().fold(0usize, |n, b| (n << 8) | *b as usize);
            frame.extend(length);
            n
        };
        if n > 524_288 || total.checked_add(n).is_none_or(|n| n > 16_777_216) {
            return Err(bad());
        }
        total += n;
        let offset = frame.len();
        frame.resize(offset + n, 0);
        reader.read_exact(&mut frame[offset..]).await?;
        let mut nodes = 0;
        ber(&frame[offset..], 0, &mut nodes)?;
        writer.write_all(&frame).await?;
        writer.flush().await?;
    }
    Err(bad())
}
fn bad() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "LDAP wire bound violated")
}
fn ber(mut b: &[u8], depth: usize, nodes: &mut usize) -> std::io::Result<()> {
    if depth > 32 {
        return Err(bad());
    }
    while !b.is_empty() {
        *nodes += 1;
        if *nodes > 20_000 || b.len() < 2 {
            return Err(bad());
        }
        let constructed = b[0] & 0x20 != 0;
        if b[0] & 0x1f == 0x1f {
            return Err(bad());
        }
        let mut offset = 2;
        let n = if b[1] & 0x80 == 0 {
            b[1] as usize
        } else {
            let size = (b[1] & 0x7f) as usize;
            if size == 0 || size > 4 || b.len() < size + 2 || b[2] == 0 {
                return Err(bad());
            }
            offset += size;
            b[2..offset]
                .iter()
                .fold(0usize, |n, v| (n << 8) | *v as usize)
        };
        if offset + n > b.len() {
            return Err(bad());
        }
        if constructed {
            ber(&b[offset..offset + n], depth + 1, nodes)?;
        }
        b = &b[offset + n..];
    }
    Ok(())
}
