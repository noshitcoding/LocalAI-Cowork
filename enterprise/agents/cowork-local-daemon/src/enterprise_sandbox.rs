//! OCI isolation is admitted only from a locally selected image and actual local engine.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Qualification {
    pub docker: PathBuf,
    pub image: String,
    pub context: String,
    pub engine_endpoint: String,
    pub image_id: String,
    pub user: String,
    pub binary_sha256: String,
}
fn command(path: &Path) -> Command {
    let mut c = Command::new(path);
    c.kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(0x08000000);
    c
}
async fn capture(mut cmd: Command) -> Result<String> {
    let out = tokio::time::timeout(Duration::from_secs(20), bounded_output(&mut cmd, 65536))
        .await
        .context("actual OCI qualification timed out")??;
    if !out.status.success() || out.stdout.len() > 65536 || out.stderr.len() > 65536 {
        bail!("actual OCI qualification failed")
    };
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}
async fn bounded_output(cmd: &mut Command, limit: usize) -> Result<std::process::Output> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child
        .stdout
        .take()
        .context("actual native stdout missing")?;
    let stderr = child
        .stderr
        .take()
        .context("actual native stderr missing")?;
    async fn read<T: tokio::io::AsyncRead + Unpin>(stream: T, limit: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        stream
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > limit {
            bail!("actual native process output exceeds bound")
        };
        Ok(bytes)
    }
    let (stdout, stderr, status) =
        tokio::try_join!(read(stdout, limit), read(stderr, limit), async {
            Ok::<_, anyhow::Error>(child.wait().await?)
        })?;
    Ok(std::process::Output {
        stdout,
        stderr,
        status,
    })
}
// Dropping a CLI future does not stop the container in the Docker engine.
// This guard requests cancellation of this exact original container only.
struct ContainerGuard {
    qualification: Qualification,
    name: String,
    finished: bool,
}
impl Drop for ContainerGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let q = self.qualification.clone();
        let name = self.name.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let mut kill = command(&q.docker);
                kill.args(["--context", &q.context, "kill", &name]);
                let _ = tokio::time::timeout(Duration::from_secs(10), kill.output()).await;
            });
        }
    }
}
impl Qualification {
    pub async fn qualify(docker: PathBuf, image: String, context: String) -> Result<Self> {
        if !docker.is_absolute()
            || !docker.is_file()
            || !image.starts_with("sha256:")
            || !crate::enterprise_policy::sha(&image[7..])
            || context.is_empty()
            || context.len() > 128
        {
            bail!("sandbox requires actual absolute Docker executable, explicit local context and immutable OCI image ID")
        };
        let docker = docker.canonicalize()?;
        if std::fs::metadata(&docker)?.len() > 64 * 1024 * 1024 {
            bail!("Docker executable exceeds qualification limit");
        }
        use sha2::Digest;
        let binary_sha256 = format!("{:x}", sha2::Sha256::digest(std::fs::read(&docker)?));
        #[cfg(unix)]
        let user = {
            let mut uid = command(Path::new("/usr/bin/id"));
            uid.arg("-u");
            let uid = capture(uid).await?;
            let mut gid = command(Path::new("/usr/bin/id"));
            gid.arg("-g");
            let gid = capture(gid).await?;
            if uid.parse::<u32>().is_err() || gid.parse::<u32>().is_err() {
                bail!("actual native UID/GID qualification failed")
            };
            format!("{uid}:{gid}")
        };
        #[cfg(not(unix))]
        let user = "1000:1000".to_owned();
        let mut inspect = command(&docker);
        inspect.args([
            "--context",
            &context,
            "context",
            "inspect",
            &context,
            "--format",
            "{{.Endpoints.docker.Host}}",
        ]);
        let endpoint = capture(inspect).await?;
        if !(endpoint == "unix:///var/run/docker.sock"
            || endpoint.starts_with("npipe:////./pipe/")
            || endpoint.starts_with("unix://")
                && Path::new(&endpoint[7..]).is_absolute()
                && Path::new(&endpoint[7..]).exists())
        {
            bail!("sandbox Docker engine must be an actual local socket or named pipe")
        }
        let mut inspect = command(&docker);
        inspect.args([
            "--context",
            &context,
            "image",
            "inspect",
            &image,
            "--format",
            "{{.Id}}",
        ]);
        let image_id = capture(inspect).await?;
        if image_id != image {
            bail!("sandbox image differs from original local approval")
        }
        let mut runtime = command(&docker);
        runtime.args([
            "--context",
            &context,
            "info",
            "--format",
            "{{json .SecurityOptions}}",
        ]);
        let options: Value = serde_json::from_str(&capture(runtime).await?)?;
        if !options.as_array().is_some_and(|items| {
            items
                .iter()
                .any(|v| v.as_str().is_some_and(|s| s.contains("seccomp")))
        }) {
            bail!("sandbox engine has no actual seccomp qualification")
        }
        Ok(Self {
            docker,
            image,
            context,
            engine_endpoint: endpoint,
            image_id,
            user,
            binary_sha256,
        })
    }
    pub async fn current(&self) -> Result<()> {
        let now = Self::qualify(
            self.docker.clone(),
            self.image.clone(),
            self.context.clone(),
        )
        .await?;
        if now.engine_endpoint != self.engine_endpoint
            || now.image_id != self.image_id
            || now.user != self.user
            || now.binary_sha256 != self.binary_sha256
        {
            bail!("actual sandbox qualification changed")
        };
        Ok(())
    }
    pub async fn shell(
        &self,
        workspace: &Path,
        script: &str,
        timeout: Duration,
        name: &str,
    ) -> Result<Value> {
        let workspace = workspace.canonicalize()?;
        let source = workspace
            .to_str()
            .context("OCI workspace path is not UTF-8")?;
        if source.contains([',', '\n', '\r']) || !workspace.is_dir() || script.len() > 65536 {
            bail!("sandbox source path or original command is invalid")
        }
        let suffix = name
            .strip_prefix("cowork-native-")
            .context("original native container name missing")?;
        if uuid::Uuid::parse_str(suffix).is_err() {
            bail!("invalid original native container name")
        };
        let mut guard = ContainerGuard {
            qualification: self.clone(),
            name: name.to_owned(),
            finished: false,
        };
        let mut run = command(&self.docker);
        run.args([
            "--context",
            &self.context,
            "run",
            "--rm",
            "--name",
            &name,
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--pids-limit",
            "128",
            "--memory",
            "1g",
            "--cpus",
            "2",
            "--user",
            &self.user,
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,noexec,size=64m",
            "--mount",
            &format!("type=bind,source={source},target=/workspace"),
            "--workdir",
            "/workspace",
            "--entrypoint",
            "/bin/sh",
            &self.image,
            "-lc",
            script,
        ]);
        let result = tokio::time::timeout(timeout, bounded_output(&mut run, 2 * 1024 * 1024)).await;
        let out = match result {
            Ok(value) => value?,
            Err(_) => {
                let mut kill = command(&self.docker);
                kill.args(["--context", &self.context, "kill", &name]);
                let _ = tokio::time::timeout(Duration::from_secs(10), kill.output()).await;
                bail!(
                    "sandbox original process deadline exceeded; external effects are not replayed"
                )
            }
        };
        guard.finished = true;
        if out.stdout.len() > 2 * 1024 * 1024 || out.stderr.len() > 2 * 1024 * 1024 {
            bail!("sandbox original output exceeds limit")
        };
        Ok(
            json!({"stdout":String::from_utf8_lossy(&out.stdout),"stderr":String::from_utf8_lossy(&out.stderr),"exit_code":out.status.code(),"original_container":name,"image_id":self.image_id,"engine_endpoint":self.engine_endpoint}),
        )
    }
}
