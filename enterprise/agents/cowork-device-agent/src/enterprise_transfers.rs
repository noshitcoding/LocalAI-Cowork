//! File I/O occurs only through current, locally approved original aliases in the user daemon.
use super::*;
pub(super) async fn once(client: &ControlPlaneClient, daemon: &LocalDaemonClient) -> Result<()> {
    let url = format!(
        "{}/api/v1/agent/executors/{}/enterprise/transfers",
        client.server_url, client.executor_id
    );
    let mut response = client
        .http
        .get(url)
        .bearer_auth(&client.token)
        .timeout(Duration::from_secs(8))
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("actual transfer discovery rejected")
    };
    let mut raw = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if raw.len() + chunk.len() > 2 * 1024 * 1024 {
            bail!("actual transfer discovery exceeds bound")
        };
        raw.extend_from_slice(&chunk)
    }
    let transfers: Vec<Value> = serde_json::from_slice(&raw)?;
    for transfer in transfers {
        if let Err(error) = one(client, daemon, &transfer).await {
            tracing::warn!(error=%error,transfer_id=?transfer["id"],"original native transfer held; no file write replay")
        }
    }
    // The ordinary list excludes completed transfers. Read the actual original
    // ACK by its retained ID after a lost completion response.
    let pending = daemon
        .call(
            "enterprise.transfers.pending",
            json!({"server_url":client.server_url}),
        )
        .await?;
    if let Some(entries) = pending.as_array() {
        for entry in entries {
            if entry["source"] == true {
                continue;
            }
            let id: Uuid = serde_json::from_value(entry["id"].clone())?;
            let response = client
                .http
                .get(format!(
                    "{}/api/v1/agent/executors/{}/enterprise/transfers/{id}",
                    client.server_url, client.executor_id
                ))
                .bearer_auth(&client.token)
                .timeout(Duration::from_secs(8))
                .send()
                .await?;
            if !response.status().is_success() {
                continue;
            }
            let bytes = response.bytes().await?;
            if bytes.len() > 65536 {
                bail!("actual original transfer recovery response exceeds bound")
            };
            let actual: Value = serde_json::from_slice(&bytes)?;
            if actual["id"] == json!(id) && !actual["receipt"].is_null() {
                let _ = daemon
                    .call(
                        "enterprise.transfers.ack",
                        json!({"transfer_id":id,"receipt":actual["receipt"]}),
                    )
                    .await;
            }
        }
    }
    Ok(())
}
async fn one(client: &ControlPlaneClient, daemon: &LocalDaemonClient, v: &Value) -> Result<()> {
    let id: Uuid = serde_json::from_value(v["id"].clone())?;
    let source = v["grant"]["source"]["id"] == json!(client.executor_id);
    let destination = v["grant"]["destination"]["id"] == json!(client.executor_id);
    if source == destination {
        bail!("transfer endpoints must be distinct actual native devices")
    };
    let binding = daemon
        .call(
            "enterprise.transfers.binding",
            json!({"transfer":v,"source":source}),
        )
        .await?;
    let pin: enterprise_policy::Pin = serde_json::from_value(binding["pin"].clone())?;
    enterprise_native::refresh(client, daemon, &pin).await?;
    let url = format!(
        "{}/api/v1/agent/executors/{}/enterprise/transfers/{id}",
        client.server_url, client.executor_id
    );
    if source {
        if v["state"] != "receiving" {
            return Ok(());
        }
        let original = daemon
            .call(
                "enterprise.transfers.original",
                json!({"transfer":v,"source":true,"server_url":client.server_url}),
            )
            .await?;
        let size = original["size"]
            .as_u64()
            .filter(|s| *s <= 16 * 1024 * 1024)
            .context("bounded original source size missing")? as usize;
        let mut bytes = Vec::with_capacity(size);
        while bytes.len() < size {
            let chunk = daemon
                .call(
                    "enterprise.transfers.chunk",
                    json!({"transfer_id":id,"offset":bytes.len(),"size":1024*1024}),
                )
                .await?;
            if chunk["offset"] != json!(bytes.len()) || chunk["source_sha256"] != v["sha256"] {
                bail!("original native source chunk differs")
            };
            let data = hex::decode(
                chunk["hex"]
                    .as_str()
                    .context("actual native original source chunk missing")?,
            )?;
            if data.is_empty() || bytes.len() + data.len() > size {
                bail!("original native source gap or overflow")
            };
            bytes.extend_from_slice(&data)
        }
        if format!("{:x}", Sha256::digest(&bytes))
            != v["sha256"].as_str().context("actual source SHA missing")?
        {
            bail!("native original source SHA differs")
        };
        enterprise_native::refresh(client, daemon, &pin).await?;
        let response = client
            .http
            .put(format!("{url}/bytes"))
            .bearer_auth(&client.token)
            .timeout(Duration::from_secs(30))
            .body(bytes)
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("actual original source PUT rejected")
        };
        let raw = response.bytes().await?;
        if raw.len() > 65536 {
            bail!("actual source receipt exceeds bound")
        };
        let receipt: Value = serde_json::from_slice(&raw)?;
        daemon
            .call(
                "enterprise.transfers.source_ack",
                json!({"transfer_id":id,"receipt":receipt}),
            )
            .await?;
        return Ok(());
    }
    if v["state"] != "ready" && v["state"] != "conflict" {
        return Ok(());
    }
    if !v["receipt"].is_null() {
        daemon
            .call(
                "enterprise.transfers.ack",
                json!({"transfer_id":id,"receipt":v["receipt"]}),
            )
            .await?;
        return Ok(());
    }
    let mut response = client
        .http
        .get(format!("{url}/bytes"))
        .bearer_auth(&client.token)
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    if !response.status().is_success()
        || response
            .headers()
            .get("x-content-sha256")
            .and_then(|h| h.to_str().ok())
            != v["sha256"].as_str()
    {
        bail!("actual original transfer download lacks its digest")
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
            bail!("actual original transfer file exceeds 16 MiB")
        };
        bytes.extend_from_slice(&chunk)
    }
    if v["size"].as_u64() != Some(bytes.len() as u64)
        || v["sha256"] != json!(format!("{:x}", Sha256::digest(&bytes)))
    {
        bail!("actual downloaded original bytes differ")
    };
    for (offset, chunk) in bytes.chunks(1024 * 1024).enumerate() {
        daemon
            .call(
                "enterprise.transfers.accept_chunk",
                json!({"transfer":v,"offset":offset*1024*1024,"hex":hex::encode(chunk)}),
            )
            .await?;
    }
    enterprise_native::refresh(client, daemon, &pin).await?;
    daemon
        .call(
            "enterprise.transfers.original",
            json!({"transfer":v,"source":false,"server_url":client.server_url}),
        )
        .await?;
    let original = daemon
        .call("enterprise.transfers.apply", json!({"transfer":v}))
        .await?;
    enterprise_native::refresh(client, daemon, &pin).await?;
    let response = client
        .http
        .post(format!("{url}/complete"))
        .bearer_auth(&client.token)
        .timeout(Duration::from_secs(8))
        .json(&original["acknowledge"])
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("original transfer complete outcome requires original receipt recovery")
    };
    let bytes = response.bytes().await?;
    if bytes.len() > 65536 {
        bail!("actual transfer ACK exceeds bound")
    };
    let receipt: Value = serde_json::from_slice(&bytes)?;
    daemon
        .call(
            "enterprise.transfers.ack",
            json!({"transfer_id":id,"receipt":receipt}),
        )
        .await?;
    Ok(())
}
