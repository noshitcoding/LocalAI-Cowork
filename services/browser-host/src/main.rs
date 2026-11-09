//! The browser starts this current-user executable. stdout is protocol only.
use base64::{Engine, engine::general_purpose::STANDARD};
use cowork_toolbox::browser_bridge::{
    self, CHUNK, MAX_MESSAGE, VERSION, read_message, write_message,
};
use serde_json::{Value, json};
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};
mod register;

async fn native_read<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Value> {
    let first = read_message(reader).await?;
    if first["type"] != "chunk" {
        return Ok(first);
    }
    let id = first["id"]
        .as_str()
        .ok_or_else(|| io::Error::other("chunk_id"))?
        .to_owned();
    let total = first["total"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= 32)
        .ok_or_else(|| io::Error::other("chunk_count"))?;
    let mut raw = Vec::new();
    let mut next = first;
    for index in 0..total {
        if next["type"] != "chunk"
            || next["id"] != id
            || next["index"] != index
            || next["total"] != total
        {
            return Err(io::Error::other("chunk_order"));
        }
        let data = next["data"]
            .as_str()
            .filter(|s| s.len() <= CHUNK * 2)
            .ok_or_else(|| io::Error::other("chunk_size"))?;
        raw.extend(
            STANDARD
                .decode(data)
                .map_err(|_| io::Error::other("chunk_encoding"))?,
        );
        if raw.len() > MAX_MESSAGE {
            return Err(io::Error::other("message_limit"));
        }
        if index + 1 < total {
            next = read_message(reader).await?;
        }
    }
    serde_json::from_slice(&raw).map_err(|_| io::Error::other("chunk_json"))
}
async fn native_write<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> io::Result<()> {
    let raw = serde_json::to_vec(value)?;
    if raw.len() > MAX_MESSAGE {
        return Err(io::Error::other("message_limit"));
    }
    if raw.len() <= CHUNK {
        return write_message(writer, value).await;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let total = raw.len().div_ceil(CHUNK);
    for (index, chunk) in raw.chunks(CHUNK).enumerate() {
        write_message(writer,&json!({"type":"chunk","id":id,"index":index,"total":total,"data":STANDARD.encode(chunk)})).await?;
    }
    Ok(())
}
async fn relay<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    hello: Value,
    input: &mut tokio::io::Stdin,
    output: &mut tokio::io::Stdout,
) -> io::Result<()> {
    write_message(&mut stream, &hello).await?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    let upstream = async {
        loop {
            let value = native_read(input).await?;
            write_message(&mut writer, &value).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), io::Error>(())
    };
    let downstream = async {
        loop {
            let value = read_message(&mut reader).await?;
            native_write(output, &value).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), io::Error>(())
    };
    tokio::select! {result=upstream=>result,result=downstream=>result}
}
async fn run() -> io::Result<()> {
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    loop {
        let request = native_read(&mut input).await?;
        if request["version"] != VERSION {
            return Err(io::Error::other("protocol_version"));
        }
        let all =
            browser_bridge::instances().map_err(|_| io::Error::other("discovery_unavailable"))?;
        match request["type"].as_str() {
            Some("instances") => {
                let public: Vec<_> = all
                    .iter()
                    .map(|v| json!({"id":v["id"],"label":v["label"]}))
                    .collect();
                native_write(
                    &mut output,
                    &json!({"type":"instances","version":VERSION,"instances":public}),
                )
                .await?;
            }
            Some("connect") => {
                let instance = all
                    .iter()
                    .find(|v| v["id"] == request["instance_id"])
                    .ok_or_else(|| io::Error::other("instance_unavailable"))?;
                let endpoint = instance["endpoint"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("endpoint_unavailable"))?;
                let hello = json!({"type":"hello","version":VERSION,"engine":request["engine"],"label":request["label"],"tabs":request["tabs"]});
                #[cfg(windows)]
                {
                    let expected = format!(
                        r"\\.\pipe\cowork-browser-{}",
                        instance["id"].as_str().unwrap_or("")
                    );
                    if endpoint != expected {
                        return Err(io::Error::other("endpoint_scope"));
                    }
                    let stream =
                        tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint)?;
                    return relay(stream, hello, &mut input, &mut output).await;
                }
                #[cfg(unix)]
                {
                    let root = browser_bridge::discovery_directory()
                        .map_err(|_| io::Error::other("discovery_unavailable"))?
                        .join("sockets");
                    if std::path::Path::new(endpoint).parent() != Some(root.as_path()) {
                        return Err(io::Error::other("endpoint_scope"));
                    }
                    let stream = tokio::net::UnixStream::connect(endpoint).await?;
                    return relay(stream, hello, &mut input, &mut output).await;
                }
            }
            _ => return Err(io::Error::other("invalid_request")),
        }
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Some(action) = std::env::args()
        .nth(1)
        .filter(|s| ["--register", "--unregister", "--diagnose"].contains(&s.as_str()))
    {
        match register::run(&action) {
            Ok(value) => println!("{value}"),
            Err(_) => {
                eprintln!("Browser host registration failed.");
                std::process::exit(1);
            }
        }
        return;
    }
    if run().await.is_err() {
        eprintln!("Cowork browser connection ended; reconnect explicitly.");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn large_native_messages_roundtrip_without_exceeding_browser_limit() {
        let (mut a, mut b) = tokio::io::duplex(65536);
        let value = json!({"text":"🦊".repeat(200000)});
        let expected = value.clone();
        let writer = tokio::spawn(async move {
            native_write(&mut a, &value).await.unwrap();
        });
        assert_eq!(native_read(&mut b).await.unwrap(), expected);
        writer.await.unwrap();
    }
    #[tokio::test]
    async fn unordered_chunks_are_rejected() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_message(
            &mut a,
            &json!({"type":"chunk","id":"x","index":1,"total":1,"data":"e30="}),
        )
        .await
        .unwrap();
        assert!(native_read(&mut b).await.is_err());
    }
}
