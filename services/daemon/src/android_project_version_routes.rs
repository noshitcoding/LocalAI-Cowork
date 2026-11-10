//! V1 versions/merge and additive project forks, backed by original scoped custody.
use super::*;
use futures_util::StreamExt;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/projects/{project}/versions", get(list).post(create))
        .route("/projects/{project}/versions/state", get(state))
        .route("/projects/{project}/snapshots", get(snapshots))
        .route("/projects/{project}/copied-content", get(templates))
        .route(
            "/projects/{project}/copied-content/{kind}/{id}",
            get(template)
                .put(edit_template)
                .layer(DefaultBodyLimit::max(16 * 1024 * 1024 + 8192)),
        )
        .route("/projects/{project}/versions/{version}/apply", post(apply))
        .route("/projects/{project}/versions/{version}/files", get(files))
        .route(
            "/projects/{project}/versions/{version}/content/{kind}/{id}",
            get(original_content),
        )
        .route(
            "/projects/{project}/versions/{version}/files/{file}/pages/{ordinal}",
            get(file_page),
        )
        .route("/projects/{project}/merge-review", get(review))
        .route(
            "/projects/{project}/merge-apply",
            post(merge).layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/projects/{project}/fork", post(fork))
        .route("/project-copy-operations/{id}", get(operation))
        .route(
            "/projects/{project}/version-operations/{key}",
            get(original_operation),
        )
        .route("/project-copy-operations/{id}/reconcile", post(reconcile))
        .route("/project-copy-operations/{id}/retire", post(retire))
}
fn original_key(headers: &HeaderMap, body: Option<&str>) -> Result<String> {
    let mut values = Vec::new();
    for name in ["idempotency-key", "x-operation-id"] {
        let mut headers = headers.get_all(name).iter();
        if let Some(value) = headers.next() {
            values.push(value.to_str().map_err(|_| invalid())?.to_owned())
        }
        if headers.next().is_some() {
            return Err(invalid());
        }
    }
    if let Some(body) = body {
        values.push(body.into());
    }
    if values.windows(2).any(|w| w[0] != w[1]) {
        return Err(Error::Conflict);
    }
    let key = values
        .into_iter()
        .next()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if key.is_empty() || key.len() > 200 || key.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(key)
}
async fn list(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(project): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_project_versions(&a, project)
            .await?
    )))
}
async fn state(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(project): Path<Uuid>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_project_version_state(&a, project)
            .await?,
    ))
}
async fn snapshots(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(project): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_project_snapshot_list(&a, project)
            .await?
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateCursor {
    after: Option<String>,
}
async fn templates(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Query(q): Query<TemplateCursor>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_project_templates(&a, p, q.after.as_deref())
            .await?,
    ))
}
async fn template(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((p, k, id)): Path<(Uuid, String, Uuid)>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_project_template(&a, p, &k, id)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditTemplate {
    expected_revision: i64,
    payload: Value,
}
async fn edit_template(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((p, k, id)): Path<(Uuid, String, Uuid)>,
    headers: HeaderMap,
    Json(q): Json<EditTemplate>,
) -> Answer {
    let key = original_key(&headers, None)?;
    Ok(Json(
        s.service
            .store()
            .android_project_template_edit(&a, p, &k, id, q.expected_revision, q.payload, &key)
            .await?,
    ))
}
async fn create(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(project): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<AndroidCreateProjectVersion>,
) -> Answer {
    let key = original_key(&headers, None)?;
    let v = match s
        .service
        .store()
        .android_project_version_begin(&a, project, body, &key, s.keys.as_ref(), s.key_id)
        .await?
    {
        cowork_storage::android_project_versions::AndroidVersionAdmission::Original(value) => value,
        cowork_storage::android_project_versions::AndroidVersionAdmission::Capture(mut capture) => {
            capture
                .capture_snapshot_files(s.service.store(), s.keys.as_ref(), s.key_id)
                .await?;
            capture.finish(s.service.store(), s.keys.as_ref()).await?
        }
    };
    Ok(Json(json!(v)))
}
async fn files(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((p, v)): Path<(Uuid, Uuid)>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_project_version_files(&a, p, v)
            .await?
    )))
}
async fn original_content(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((p, v, kind, id)): Path<(Uuid, Uuid, String, Uuid)>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_project_version_object(&a, p, v, &kind, id, s.keys.as_ref())
            .await?
    )))
}
async fn file_page(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((p, v, f, o)): Path<(Uuid, Uuid, Uuid, u32)>,
) -> std::result::Result<Response, Failure> {
    let bytes = s
        .service
        .store()
        .android_project_version_file_page(&a, p, v, f, o, s.keys.as_ref())
        .await?
        .ok_or(Error::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes.to_vec(),
    )
        .into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MergeQuery {
    base_version_id: Uuid,
    current_version_id: Uuid,
    result_version_id: Uuid,
}
async fn review(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Query(q): Query<MergeQuery>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_project_merge_review(
                &a,
                p,
                q.base_version_id,
                q.current_version_id,
                q.result_version_id,
                s.keys.as_ref()
            )
            .await?
    )))
}
async fn copy_objects(
    s: &CompatState,
    a: &Principal,
    p: &cowork_storage::android_project_versions::AndroidProjectCopy,
) -> Result<()> {
    if p.original_receipt().is_some() {
        return Ok(());
    }
    for (path, _) in p.files() {
        match s
            .service
            .store()
            .android_project_copy_admit(a, p.id(), &path, s.key_id)
            .await
        {
            Ok(Some(permit)) => {
                let held = s.clone();
                let actor = a.clone();
                let id = p.id();
                let source_path = path.clone();
                let stream = futures_util::stream::try_unfold(0u32, move |ordinal| {
                    let s = held.clone();
                    let a = actor.clone();
                    let path = source_path.clone();
                    async move {
                        let page = s
                            .service
                            .store()
                            .android_project_copy_page(&a, id, &path, ordinal, s.keys.as_ref())
                            .await?;
                        match page {
                            Some(bytes) => Ok(Some((
                                bytes.to_vec(),
                                ordinal.checked_add(1).ok_or(Error::BudgetExceeded)?,
                            ))),
                            None => Ok(None),
                        }
                    }
                });
                // An error/cancel after this original permit leaves its durable dispatched marker.
                // A future request must perform GET reconciliation; it cannot obtain another PUT.
                let actual = s
                    .objects
                    .put_stream(permit.target(), Box::pin(stream))
                    .await?;
                s.service
                    .store()
                    .android_project_copy_received(permit, &actual)
                    .await?;
            }
            Ok(None) => {}
            Err(Error::UncertainEffect) => reconcile_file(s, a, p.id(), &path).await?,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
async fn reconcile_file(s: &CompatState, a: &Principal, id: Uuid, path: &str) -> Result<()> {
    let mut witness = s
        .service
        .store()
        .android_project_copy_read_witness(a, id, path)
        .await?;
    let mut source = s.objects.get_stream(witness.target()).await?;
    while let Some(page) = source.next().await {
        witness.observe(&page?)?;
    }
    s.service
        .store()
        .android_project_copy_confirm_read(a, witness)
        .await
}
async fn apply(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((project, version)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<AndroidApplyProjectVersion>,
) -> Answer {
    let key = original_key(&headers, None)?;
    let p = s
        .service
        .store()
        .android_project_apply_prepare(
            &a,
            project,
            version,
            body,
            &key,
            s.objects.backend_id(),
            s.key_id,
        )
        .await?;
    copy_objects(&s, &a, &p).await?;
    Ok(Json(json!(
        s.service
            .store()
            .android_project_copy_commit(&a, p.id(), s.keys.as_ref(), s.key_id)
            .await?
    )))
}
async fn merge(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(project): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<AndroidApplyProjectMerge>,
) -> Answer {
    let key = original_key(&headers, None)?;
    let p = s
        .service
        .store()
        .android_project_merge_prepare(
            &a,
            project,
            body,
            &key,
            s.objects.backend_id(),
            s.keys.as_ref(),
            s.key_id,
        )
        .await?;
    copy_objects(&s, &a, &p).await?;
    Ok(Json(json!(
        s.service
            .store()
            .android_project_copy_commit(&a, p.id(), s.keys.as_ref(), s.key_id)
            .await?
    )))
}
async fn fork(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(project): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<AndroidForkProject>,
) -> Answer {
    original_key(&headers, Some(&body.idempotency_key))?;
    let p = s
        .service
        .store()
        .android_project_fork_prepare(&a, project, body, s.objects.backend_id(), s.key_id)
        .await?;
    copy_objects(&s, &a, &p).await?;
    Ok(Json(json!(
        s.service
            .store()
            .android_project_fork_commit(&a, p.id(), s.keys.as_ref(), s.key_id)
            .await?
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyCursor {
    after: Option<String>,
}
async fn original_operation(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((project, key)): Path<(Uuid, String)>,
) -> Answer {
    let id = s
        .service
        .store()
        .android_project_copy_original(&a, project, &key)
        .await?;
    Ok(Json(
        s.service
            .store()
            .android_project_copy_summary(&a, id, None)
            .await?,
    ))
}
async fn operation(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<CopyCursor>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_project_copy_summary(&a, id, q.after.as_deref())
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reconcile {
    paths: Vec<String>,
}
async fn reconcile(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(body): Json<Reconcile>,
) -> Answer {
    if body.paths.len() > 2000 {
        return Err(invalid().into());
    }
    for path in body.paths {
        reconcile_file(&s, &a, id, &path).await?;
    }
    operation(
        State(s),
        Session(a),
        Path(id),
        Query(CopyCursor { after: None }),
    )
    .await
}
async fn retire(State(s): State<CompatState>, Session(a): Session, Path(id): Path<Uuid>) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_project_copy_retire(&a, id)
            .await?,
    ))
}
