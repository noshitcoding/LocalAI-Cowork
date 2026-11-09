//! Durable V1 wire feed. Read capture does not invoke providers, tools or schedules.
use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/sync/changes", get(pull).post(push))
        .route("/sync/events", get(stream))
}
async fn push(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(v): Json<AndroidWireSyncBatch>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_wire_sync_push(&a, v.changes)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeedQuery {
    #[serde(default)]
    after: i64,
    #[serde(default)]
    limit: Option<i64>,
}
async fn pull(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<FeedQuery>,
) -> Answer {
    capture(&s, &a).await?;
    Ok(Json(
        s.service
            .store()
            .android_wire_sync_pull(&a, q.after, q.limit.unwrap_or(200).clamp(1, 1000))
            .await?,
    ))
}
fn id(value: &Value) -> Result<Uuid> {
    value["id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(invalid)
}
async fn capture_one(
    s: &CompatState,
    a: &Principal,
    kind: &str,
    team: Option<Uuid>,
    project: Option<Uuid>,
    v: Value,
    tombstone: bool,
) -> Result<()> {
    s.service
        .store()
        .android_wire_sync_capture(
            a,
            kind,
            id(&v)?,
            team,
            project,
            v["revision"].as_i64().ok_or_else(invalid)?,
            v,
            tombstone,
        )
        .await
}
async fn capture_run(
    s: &CompatState,
    a: &Principal,
    team: Option<Uuid>,
    project: Uuid,
    run: Value,
) -> Result<usize> {
    let id = run["spec"]["id"]
        .as_str()
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(invalid)?;
    let revision = run["revision"]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or_else(invalid)?;
    s.service
        .store()
        .android_wire_sync_capture(
            a,
            "run",
            id,
            team,
            Some(project),
            revision,
            run.clone(),
            false,
        )
        .await?;
    let user = wire_message(
        &run,
        "user",
        json!(prompt(&run["spec"]["input"])?),
        run["spec"]["creator_user_id"]
            .as_str()
            .and_then(|v| Uuid::parse_str(v).ok()),
    )?;
    capture_one(s, a, "message", team, Some(project), user, false).await?;
    if let Some(content) = run["result"]["content"]
        .as_str()
        .or_else(|| run["result"].as_str())
    {
        capture_one(
            s,
            a,
            "message",
            team,
            Some(project),
            wire_message(&run, "assistant", json!(content), None)?,
            false,
        )
        .await?;
        Ok(3)
    } else {
        Ok(2)
    }
}
pub(super) async fn capture(s: &CompatState, a: &Principal) -> Result<()> {
    let mut captured = 0usize;
    for v in s.service.store().android_personal_projects(a).await? {
        let project = id(&v)?;
        capture_one(s, a, "project", None, Some(project), v, false).await?;
        captured += 1;
        for thread in s
            .service
            .store()
            .android_personal_threads(a, project)
            .await?
        {
            let thread_id = id(&thread)?;
            capture_one(s, a, "thread", None, Some(project), thread, false).await?;
            captured += 1;
            for message in s
                .service
                .store()
                .android_personal_messages(a, thread_id)
                .await?
            {
                capture_one(s, a, "message", None, Some(project), message, false).await?;
                captured += 1;
            }
        }
        for run in s.service.store().android_personal_runs(a, project).await? {
            captured += capture_run(s, a, None, project, run).await?;
        }
        for task in s.service.store().android_tasks(a, project).await? {
            capture_one(s, a, "task", None, Some(project), task, false).await?;
            captured += 1;
        }
        for schedule in s.service.store().android_schedules(a, project).await? {
            let value = serde_json::to_value(&schedule).map_err(|_| invalid())?;
            capture_one(s, a, "schedule", None, Some(project), value, false).await?;
            captured += 1;
        }
        let mut after = None;
        loop {
            let page = s
                .service
                .store()
                .android_workflows(a, project, after, 25)
                .await?;
            for entry in page.items {
                let mut metadata = json!({"schema_version":2,"id":entry.id,"revision":entry.revision,"project_id":project,"owner_user_id":a.identity_id(),"name":entry.name,"engine":entry.engine,"revoked":entry.revoked,"publication_grant":false,"execution_grant":false,"credential_grant":false});
                if let Some(version) = entry.published_version {
                    let original = s
                        .service
                        .store()
                        .android_workflow_version(a, project, entry.id, version.id, s.keys.as_ref())
                        .await?;
                    let raw = serde_json::to_vec(&original.definition).map_err(|_| invalid())?;
                    if hash(&raw) != version.definition_sha256 {
                        return Err(invalid());
                    }
                    metadata["definition_original"] = json!({"version_id":version.id,"sha256":version.definition_sha256,"size":raw.len(),"path":format!("/api/v1/private-workflows/{}/versions/{}?project_id={project}",entry.id,version.id)});
                    if raw.len() <= 256 * 1024 {
                        metadata["definition"] = original.definition;
                    }
                }
                capture_one(s, a, "crew", None, Some(project), metadata, entry.revoked).await?;
                captured += 1;
            }
            if captured > 10000 {
                return Err(Error::BudgetExceeded);
            }
            let Some(next) = page.next_cursor else { break };
            after = Some(next);
        }
        if captured > 10_000 {
            return Err(Error::BudgetExceeded);
        }
    }
    for project in all_projects(s, a).await? {
        capture_one(
            s,
            a,
            "project",
            Some(project.team_id),
            Some(project.id),
            project_wire(s, a, &project).await?,
            project.archived,
        )
        .await?;
        captured += 1;
        if project.archived {
            continue;
        }
        for order in s.service.orders(a, project.team_id, project.id).await? {
            capture_one(
                s,
                a,
                "thread",
                Some(project.team_id),
                Some(project.id),
                thread_wire(&order),
                order.archived,
            )
            .await?;
            captured += 1;
        }
        for task in s.service.store().android_tasks(a, project.id).await? {
            capture_one(
                s,
                a,
                "task",
                Some(project.team_id),
                Some(project.id),
                task,
                false,
            )
            .await?;
            captured += 1;
        }
        for run in s.service.runs(a, project.team_id, project.id).await? {
            let wire = run_wire(s, a, &run).await?;
            captured += capture_run(s, a, Some(project.team_id), project.id, wire).await?;
        }
        for run in s
            .service
            .store()
            .android_device_runs(a, project.team_id, project.id)
            .await?
        {
            captured += capture_run(s, a, Some(project.team_id), project.id, run).await?;
        }
        // Preserve the actual native recurrence and its grant binding. A cron expression is
        // never invented for native Once/Interval schedules during read-only sync capture.
        for schedule in s
            .service
            .store()
            .team_schedules(a, project.team_id, project.id)
            .await?
        {
            let value = serde_json::to_value(&schedule).map_err(|_| invalid())?;
            capture_one(
                s,
                a,
                "schedule",
                Some(project.team_id),
                Some(project.id),
                value,
                false,
            )
            .await?;
            captured += 1;
        }
        if captured > 10_000 {
            return Err(Error::BudgetExceeded);
        }
    }
    for provider in provider_inventory(s, a).await? {
        let team = if provider["team_id"].is_null() {
            None
        } else {
            Some(
                provider["team_id"]
                    .as_str()
                    .and_then(|v| Uuid::parse_str(v).ok())
                    .ok_or_else(invalid)?,
            )
        };
        capture_one(s, a, "provider_profile", team, None, provider, false).await?;
        captured += 1;
    }
    for kind in ["skill", "memory", "mcp_metadata"] {
        let mut after = None;
        loop {
            let page = s
                .service
                .sync_metadata_snapshot(a, kind, after, 1000)
                .await?;
            for change in &page.changes {
                let entity = &change.payload;
                let mut payload = entity["payload"].clone();
                if kind == "skill" {
                    payload["prompt_template"] = payload["content"].clone();
                }
                if kind == "memory" {
                    payload["key"] = payload["name"].clone();
                    payload["scope"] = json!("project");
                    payload["scope_ref"] = payload["project_id"].clone();
                }
                if kind == "mcp_metadata" {
                    payload["transport"] = json!("streamable_http");
                    payload["executable_hint"] = payload["endpoint"].clone();
                    payload["device_binding_required"] = json!(true);
                }
                let team = payload["team_id"]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or_else(invalid)?;
                let project = payload["project_id"]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok());
                s.service
                    .store()
                    .android_wire_sync_capture(
                        a,
                        kind,
                        change.entity_id,
                        Some(team),
                        project,
                        entity["revision"].as_i64().ok_or_else(invalid)?,
                        payload,
                        change.tombstone,
                    )
                    .await?;
                captured += 1;
            }
            if captured > 10_000 {
                return Err(Error::BudgetExceeded);
            }
            if !page.has_more {
                break;
            }
            after = page.changes.last().map(|c| c.entity_id);
            if after.is_none() {
                return Err(Error::Conflict);
            }
        }
    }
    Ok(())
}
async fn stream(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
) -> std::result::Result<Response, Failure> {
    let after = headers
        .get("last-event-id")
        .and_then(|s| s.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    s.service
        .store()
        .android_wire_sync_pull(&a, after, 1)
        .await?;
    let source = futures_util::stream::unfold(
        (s, a, after, VecDeque::<Value>::new()),
        |(s, a, mut after, mut pending)| async move {
            if pending.is_empty() {
                loop {
                    capture(&s, &a).await.ok()?;
                    let page = s
                        .service
                        .store()
                        .android_wire_sync_pull(&a, after, 200)
                        .await
                        .ok()?;
                    after = page["next_cursor"].as_i64()?;
                    pending.extend(page["changes"].as_array()?.iter().cloned());
                    if !pending.is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(4)).await;
                }
            }
            let change = pending.pop_front()?;
            let cursor = change["cursor"].as_i64()?;
            let change = s
                .service
                .store()
                .android_wire_sync_frame(&a, cursor)
                .await
                .ok()??;
            let frame = Event::default()
                .id(cursor.to_string())
                .event("sync_change")
                .json_data(change)
                .ok()?;
            Some((Ok::<_, Infallible>(frame), (s, a, after, pending)))
        },
    );
    Ok(Sse::new(source)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
        .into_response())
}
