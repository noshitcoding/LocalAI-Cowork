//! Original, one-shot streamed object copies. The caller owns the durable admission journal.
use super::*;
use futures_util::{Stream, StreamExt};
use sha2::Digest;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub(crate) type ObjectPages = Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send>>;
const PAGE: usize = 1024 * 1024;
fn managed_current(managed: &Managed) -> Result<()> {
    let current = open_directory(&managed.root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let original = managed._held.metadata().map_err(|_| denied())?;
        let actual = current.metadata().map_err(|_| denied())?;
        if original.dev() != actual.dev() || original.ino() != actual.ino() {
            return Err(denied());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut original: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let mut actual: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(managed._held.as_raw_handle(), &mut original) } == 0
            || unsafe { GetFileInformationByHandle(current.as_raw_handle(), &mut actual) } == 0
            || original.dwVolumeSerialNumber != actual.dwVolumeSerialNumber
            || original.nFileIndexHigh != actual.nFileIndexHigh
            || original.nFileIndexLow != actual.nFileIndexLow
        {
            return Err(denied());
        }
    }
    Ok(())
}
fn object_current(managed: &Managed, name: &str, held: &std::fs::File) -> Result<()> {
    managed_current(managed)?;
    let actual = open_private(&managed.root.join(name))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let a = actual.metadata().map_err(|_| denied())?;
        let h = held.metadata().map_err(|_| denied())?;
        if a.dev() != h.dev() || a.ino() != h.ino() {
            return Err(denied());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut a: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let mut h: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(actual.as_raw_handle(), &mut a) } == 0
            || unsafe { GetFileInformationByHandle(held.as_raw_handle(), &mut h) } == 0
            || a.dwVolumeSerialNumber != h.dwVolumeSerialNumber
            || a.nFileIndexHigh != h.nFileIndexHigh
            || a.nFileIndexLow != h.nFileIndexLow
        {
            return Err(denied());
        }
    }
    Ok(())
}

struct CheckedPages {
    source: ObjectPages,
    seen: u64,
    hash: Sha256,
    expected: String,
    size: u64,
    done: bool,
}
impl CheckedPages {
    async fn next(&mut self) -> Result<Option<Vec<u8>>> {
        if self.done {
            return Ok(None);
        }
        let item = self.source.next().await.transpose()?;
        let Some(bytes) = item else {
            if self.seen != self.size || hex(&self.hash.clone().finalize()) != self.expected {
                return Err(unavailable());
            }
            self.done = true;
            return Ok(None);
        };
        if bytes.is_empty() || bytes.len() > PAGE {
            return Err(unavailable());
        }
        let n = self
            .seen
            .checked_add(bytes.len() as u64)
            .ok_or_else(unavailable)?;
        if n > self.size {
            return Err(unavailable());
        }
        self.hash.update(&bytes);
        self.seen = n;
        if n == self.size {
            if hex(&self.hash.clone().finalize()) != self.expected
                || self.source.next().await.transpose()?.is_some()
            {
                return Err(unavailable());
            }
            self.done = true;
        }
        Ok(Some(bytes))
    }
}
impl S3Store {
    pub(crate) async fn put_stream(
        &self,
        v: &TeamBlobVersion,
        source: ObjectPages,
    ) -> Result<serde_json::Value> {
        self.validate_version(v)?;
        let checked = CheckedPages {
            source,
            seen: 0,
            hash: Sha256::new(),
            expected: v.sha256.clone(),
            size: v.size,
            done: false,
        };
        match &self.backend {
            Backend::Managed(managed) => {
                managed_current(managed)?;
                // New immutable original key only. A failed/uncertain copy never overwrites it.
                let file = create_private(&managed.root.join(hash(v.object_key.as_bytes())))?;
                let mut file = tokio::fs::File::from_std(file);
                let mut checked = checked;
                while let Some(bytes) = checked.next().await? {
                    file.write_all(&bytes).await.map_err(|_| unavailable())?;
                }
                file.flush().await.map_err(|_| unavailable())?;
                file.sync_all().await.map_err(|_| unavailable())?;
                #[cfg(unix)]
                managed._held.sync_all().map_err(|_| unavailable())?;
                managed_current(managed)?;
                Ok(
                    serde_json::json!({"format":"project-version-object-put/1","backend_id":self.id,"object_key":v.object_key,"sha256":v.sha256,"size":v.size,"response":"managed_file_synced"}),
                )
            }
            Backend::Remote(remote) => {
                let stream = futures_util::stream::try_unfold(checked, |mut state| async move {
                    state
                        .next()
                        .await
                        .map(|next| next.map(|bytes| (bytes, state)))
                        .map_err(|_| std::io::Error::other("original_object_stream_failed"))
                });
                let response = remote
                    .project_stream_request(v, reqwest::Body::wrap_stream(stream))
                    .await?;
                if !response.status().is_success() {
                    return Err(unavailable());
                }
                Ok(
                    serde_json::json!({"format":"project-version-object-put/1","backend_id":self.id,"object_key":v.object_key,"sha256":v.sha256,"size":v.size,"response_status":response.status().as_u16()}),
                )
            }
        }
    }
    /// Used for original copy-ACK reconciliation. It never sends PUT or adopts another object.
    pub(crate) async fn get_stream(&self, v: &TeamBlobVersion) -> Result<ObjectPages> {
        self.validate_version(v)?;
        enum Reader {
            Managed(tokio::fs::File, Managed, String, std::fs::File),
            Remote(reqwest::Response, Vec<u8>, usize),
        }
        let reader = match &self.backend {
            Backend::Managed(managed) => {
                managed_current(managed)?;
                let file = open_private(&managed.root.join(hash(v.object_key.as_bytes())))?;
                if file.metadata().map_err(|_| unavailable())?.len() != v.size {
                    return Err(unavailable());
                }
                let held = file.try_clone().map_err(|_| denied())?;
                let original = Managed {
                    root: managed.root.clone(),
                    _held: managed._held.try_clone().map_err(|_| denied())?,
                };
                Reader::Managed(
                    tokio::fs::File::from_std(file),
                    original,
                    hash(v.object_key.as_bytes()),
                    held,
                )
            }
            Backend::Remote(remote) => {
                let response = remote.request("GET", v, Vec::new()).await?;
                if !response.status().is_success()
                    || response.content_length().is_some_and(|n| n != v.size)
                {
                    return Err(unavailable());
                }
                Reader::Remote(response, Vec::new(), 0)
            }
        };
        let expected = v.sha256.clone();
        let size = v.size;
        let stream = futures_util::stream::try_unfold(
            (reader, 0u64, Sha256::new(), false),
            move |(mut reader, seen, mut h, done)| {
                let expected = expected.clone();
                async move {
                    if done {
                        return Ok(None);
                    }
                    let bytes = match &mut reader {
                        Reader::Managed(file, managed, name, held) => {
                            object_current(managed, name, held)?;
                            let mut b = vec![0; PAGE];
                            let n = file.read(&mut b).await.map_err(|_| unavailable())?;
                            b.truncate(n);
                            b
                        }
                        Reader::Remote(response, pending, offset) => {
                            while *offset == pending.len() {
                                match response.chunk().await.map_err(|_| unavailable())? {
                                    Some(bytes) => {
                                        if bytes.len() as u64 > TEAM_BLOB_MAX_BYTES {
                                            return Err(unavailable());
                                        }
                                        *pending = bytes.to_vec();
                                        *offset = 0;
                                    }
                                    None => {
                                        pending.clear();
                                        *offset = 0;
                                        break;
                                    }
                                }
                            }
                            let n = (pending.len() - *offset).min(PAGE);
                            let bytes = pending[*offset..*offset + n].to_vec();
                            *offset += n;
                            bytes
                        }
                    };
                    if bytes.is_empty() {
                        if seen != size || hex(&h.finalize()) != expected {
                            return Err(unavailable());
                        }
                        return Ok(None);
                    }
                    let next = seen
                        .checked_add(bytes.len() as u64)
                        .ok_or_else(unavailable)?;
                    if next > size {
                        return Err(unavailable());
                    }
                    h.update(&bytes);
                    // SHA is checked before the last declared byte block is yielded. A subsequent EOF
                    // is still required by callers before they record a successful original read.
                    if next == size && hex(&h.clone().finalize()) != expected {
                        return Err(unavailable());
                    }
                    Ok(Some((bytes, (reader, next, h, false))))
                }
            },
        );
        Ok(Box::pin(stream))
    }
}
impl Remote {
    /// Same signed canonical request as the ordinary backend, using the already authenticated
    /// immutable original body hash and length rather than collecting file bytes in memory.
    async fn project_stream_request(
        &self,
        v: &TeamBlobVersion,
        body: reqwest::Body,
    ) -> Result<reqwest::Response> {
        let mut url = self.endpoint.clone();
        url.set_path(&format!("/{}/{}", self.bucket, v.object_key));
        let now = chrono::Utc::now();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().ok_or_else(denied)?),
            None => url.host_str().ok_or_else(denied)?.to_owned(),
        };
        let headers = format!(
            "host:{host}\nx-amz-content-sha256:{}\nx-amz-date:{timestamp}\nx-amz-meta-sha256:{}\n",
            v.sha256, v.sha256
        );
        let signed = "host;x-amz-content-sha256;x-amz-date;x-amz-meta-sha256";
        let canonical = format!("PUT\n{}\n\n{headers}\n{signed}\n{}", url.path(), v.sha256);
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hash(canonical.as_bytes())
        );
        let prefix = Zeroizing::new(format!("AWS4{}", self.secret_key.as_str()));
        let date_key = Zeroizing::new(hmac(prefix.as_bytes(), date.as_bytes())?);
        let region_key = Zeroizing::new(hmac(&date_key, self.region.as_bytes())?);
        let service_key = Zeroizing::new(hmac(&region_key, b"s3")?);
        let signing_key = Zeroizing::new(hmac(&service_key, b"aws4_request")?);
        let signature = hex(&hmac(&signing_key, to_sign.as_bytes())?);
        let authorization = Zeroizing::new(format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}",
            self.access_key.as_str()
        ));
        self.client
            .put(url)
            .header("host", host)
            .header("x-amz-content-sha256", &v.sha256)
            .header("x-amz-date", timestamp)
            .header("x-amz-meta-sha256", &v.sha256)
            .header("authorization", authorization.as_str())
            .header("content-type", "application/octet-stream")
            .header("content-length", v.size)
            .body(body)
            .send()
            .await
            .map_err(|_| unavailable())
    }
}
