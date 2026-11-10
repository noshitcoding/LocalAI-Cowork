//! Original V1 snapshot protocol for private identity-owned initial data.
use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route(
            "/snapshots",
            post(begin).layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/snapshots/{snapshot}", get(manifest))
        .route("/snapshots/{snapshot}/upload", get(status))
        .route(
            "/snapshots/{snapshot}/chunks/{digest}",
            get(download)
                .put(upload)
                .layer(DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
        .route("/snapshots/{snapshot}/commit", post(commit))
        .route("/projects/{project}/files", get(files))
}
async fn begin(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Answer {
    let key = headers
        .get("idempotency-key")
        .map(|v| v.to_str().map_err(|_| invalid()))
        .transpose()?;
    let operation = headers
        .get("x-operation-id")
        .map(|v| v.to_str().map_err(|_| invalid()))
        .transpose()?;
    if key.is_some() && operation.is_some() && key != operation {
        return Err(Error::Conflict.into());
    }
    let key = key.or(operation);
    if let Some(value) = s
        .service
        .store()
        .android_project_snapshot_begin(&a, body.clone(), key, s.keys.as_ref(), s.key_id)
        .await?
    {
        return Ok(Json(value));
    }
    Ok(Json(
        s.service
            .store()
            .android_private_snapshot_begin(&a, body, key, s.key_id)
            .await?,
    ))
}
async fn manifest(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
) -> Answer {
    if let Some(value) = s
        .service
        .store()
        .android_project_snapshot(&a, id, false)
        .await?
    {
        return Ok(Json(value));
    }
    Ok(Json(
        s.service
            .store()
            .android_private_snapshot(&a, id, false)
            .await?,
    ))
}
async fn status(State(s): State<CompatState>, Session(a): Session, Path(id): Path<Uuid>) -> Answer {
    if let Some(value) = s
        .service
        .store()
        .android_project_snapshot(&a, id, true)
        .await?
    {
        return Ok(Json(value));
    }
    Ok(Json(
        s.service
            .store()
            .android_private_snapshot(&a, id, true)
            .await?,
    ))
}
async fn commit(State(s): State<CompatState>, Session(a): Session, Path(id): Path<Uuid>) -> Answer {
    if let Some(value) = s
        .service
        .store()
        .android_project_snapshot_commit(&a, id, s.keys.as_ref())
        .await?
    {
        return Ok(Json(value));
    }
    Ok(Json(
        s.service
            .store()
            .android_private_snapshot_commit(&a, id, s.keys.as_ref())
            .await?,
    ))
}
async fn files(State(s): State<CompatState>, Session(a): Session, Path(id): Path<Uuid>) -> Answer {
    Ok(Json(json!(
        s.service.store().android_private_files(&a, id).await?
    )))
}
async fn upload(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((id, digest)): Path<(Uuid, String)>,
    bytes: Bytes,
) -> Answer {
    if let Some((value, _)) = s
        .service
        .store()
        .android_project_snapshot_chunk(&a, id, &digest, Some(&bytes), s.keys.as_ref(), s.key_id)
        .await?
    {
        return Ok(Json(value));
    }
    Ok(Json(
        s.service
            .store()
            .android_private_snapshot_chunk(
                &a,
                id,
                &digest,
                Some(&bytes),
                s.keys.as_ref(),
                s.key_id,
            )
            .await?
            .0,
    ))
}
async fn download(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((id, digest)): Path<(Uuid, String)>,
) -> std::result::Result<Response, Failure> {
    if let Some((_, bytes)) = s
        .service
        .store()
        .android_project_snapshot_chunk(&a, id, &digest, None, s.keys.as_ref(), s.key_id)
        .await?
    {
        return Ok((
            [(header::CONTENT_TYPE, "application/octet-stream")],
            bytes.to_vec(),
        )
            .into_response());
    }
    let bytes = s
        .service
        .store()
        .android_private_snapshot_chunk(&a, id, &digest, None, s.keys.as_ref(), s.key_id)
        .await?
        .1;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}
