//! Independent content copies. Source run IDs and grants are provenance, never active authority.
use super::*;

pub(super) async fn thread_messages(
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    project: &ProjectScope,
    thread: Uuid,
) -> Result<Vec<Value>> {
    let mut values: BTreeMap<Uuid, Value> = BTreeMap::new();
    let stored = if project.team.is_none() {
        "SELECT id,CASE WHEN LENGTH(payload)<=16777216 THEN payload ELSE NULL END AS payload FROM android_personal_messages WHERE project_id=$1 AND thread_id=$2 AND deleted=0 AND id>$3 ORDER BY id LIMIT 1"
    } else {
        "SELECT id,CASE WHEN LENGTH(payload)<=16777216 THEN payload ELSE NULL END AS payload FROM android_project_thread_history WHERE project_id=$1 AND thread_id=$2 AND id>$3 ORDER BY id LIMIT 1"
    };
    let mut after = String::new();
    let mut bytes = 0u64;
    loop {
        let Some(row) = sqlx::query(stored)
            .bind(project.project.to_string())
            .bind(thread.to_string())
            .bind(&after)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
        else {
            break;
        };
        let id = row_id(&row, "id")?;
        after = id.to_string();
        let encoded = row
            .try_get::<Option<String>, _>("payload")
            .map_err(storage_error)?
            .ok_or(Error::BudgetExceeded)?;
        bytes = bytes
            .checked_add(encoded.len() as u64)
            .ok_or(Error::BudgetExceeded)?;
        if bytes > content::MAX_CONTENT || values.len() >= 10000 {
            return Err(Error::BudgetExceeded);
        }
        values.insert(id, parse(&encoded)?);
    }
    let sql = if project.team.is_none() {
        "SELECT id,CASE WHEN LENGTH(payload)<=16777216 THEN payload ELSE NULL END AS payload FROM android_scoped_runs WHERE project_id=$1 AND order_id=$2 AND creator_id=$3 AND team_id IS NULL AND id>$4 UNION ALL SELECT id,CASE WHEN LENGTH(payload)<=16777216 THEN payload ELSE NULL END AS payload FROM android_private_server_runs WHERE project_id=$1 AND thread_id=$2 AND owner_id=$3 AND id>$4 ORDER BY id LIMIT 1"
    } else {
        "SELECT id,CASE WHEN LENGTH(payload)<=16777216 THEN payload ELSE NULL END AS payload FROM android_scoped_runs WHERE project_id=$1 AND order_id=$2 AND team_id IS NOT NULL AND CAST($3 AS TEXT)=CAST($3 AS TEXT) AND id>$4 UNION ALL SELECT id,CASE WHEN LENGTH(payload)<=16777216 THEN payload ELSE NULL END AS payload FROM team_runs WHERE project_id=$1 AND order_id=$2 AND CAST($3 AS TEXT)=CAST($3 AS TEXT) AND id>$4 ORDER BY id LIMIT 1"
    };
    after.clear();
    let mut runs = 0usize;
    loop {
        let Some(row) = sqlx::query(sql)
            .bind(project.project.to_string())
            .bind(thread.to_string())
            .bind(actor.identity_id().to_string())
            .bind(&after)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
        else {
            break;
        };
        after = row.try_get("id").map_err(storage_error)?;
        runs += 1;
        if runs > 5000 {
            return Err(Error::BudgetExceeded);
        }
        let encoded = row
            .try_get::<Option<String>, _>("payload")
            .map_err(storage_error)?
            .ok_or(Error::BudgetExceeded)?;
        bytes = bytes
            .checked_add(encoded.len() as u64)
            .ok_or(Error::BudgetExceeded)?;
        if bytes > content::MAX_CONTENT {
            return Err(Error::BudgetExceeded);
        }
        let value: Value = parse(&encoded)?;
        let run = value
            .pointer("/spec/id")
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or(Error::Conflict)?;
        let created = value
            .pointer("/spec/created_at")
            .cloned()
            .or_else(|| {
                value
                    .get("created_at")
                    .and_then(Value::as_i64)
                    .and_then(|v| device_iso(v).ok())
                    .map(Value::String)
            })
            .ok_or(Error::Conflict)?;
        let input = value
            .pointer("/spec/input/prompt")
            .or_else(|| value.pointer("/spec/input"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let input = if let Some(input) = input {
            input
        } else {
            let request: String =
                sqlx::query_scalar("SELECT request FROM team_runs WHERE id=$1 AND project_id=$2")
                    .bind(run.to_string())
                    .bind(project.project.to_string())
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(storage_error)?;
            let request: Value = parse(&request)?;
            request["prompt"]
                .as_str()
                .ok_or(Error::Conflict)?
                .to_owned()
        };
        let updated = value
            .pointer("/updated_at")
            .cloned()
            .filter(Value::is_string)
            .unwrap_or_else(|| created.clone());
        for (id, role, text) in [
            (run, "user", Some(input)),
            (
                Uuid::from_u128(run.as_u128() ^ 1),
                "assistant",
                value
                    .pointer("/result/content")
                    .or_else(|| value.get("response"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ),
        ] {
            if let Some(text) = text {
                let timestamp = if role == "assistant" {
                    &updated
                } else {
                    &created
                };
                let author = if role == "user" {
                    value
                        .pointer("/spec/creator_user_id")
                        .or_else(|| value.get("creator_id"))
                        .cloned()
                        .unwrap_or(Value::Null)
                } else {
                    Value::Null
                };
                let v = json!({"schema_version":2,"id":id,"revision":1,"etag":hash(text.as_bytes()),"thread_id":thread,"author_user_id":author,"role":role,"content":text,"run_id":run,"created_at":timestamp,"updated_at":timestamp,"deleted_at":null});
                values.insert(id, v);
            }
        }
    }
    if values.len() > 10000 {
        return Err(Error::BudgetExceeded);
    }
    let mut list: Vec<_> = values.into_values().collect();
    list.sort_by_key(|v| {
        (
            v["created_at"].as_str().unwrap_or_default().to_owned(),
            v["id"].as_str().unwrap_or_default().to_owned(),
        )
    });
    Ok(list)
}
async fn source_thread(
    tx: &mut Transaction<'_, Any>,
    project: &ProjectScope,
    id: Uuid,
) -> Result<Value> {
    let sql = if project.team.is_none() {
        "SELECT payload FROM android_personal_threads WHERE id=$1 AND project_id=$2 AND deleted=0"
    } else {
        "SELECT payload FROM team_orders WHERE id=$1 AND project_id=$2"
    };
    let value: String = sqlx::query_scalar(sql)
        .bind(id.to_string())
        .bind(project.project.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    let value: Value = parse(&value)?;
    if value["project_id"] != json!(project.project) || value.get("archived") == Some(&json!(true))
    {
        return Err(Error::Conflict);
    }
    Ok(value)
}
impl Store {
    pub async fn android_project_fork_thread(
        &self,
        actor: &Principal,
        input: AndroidForkThread,
    ) -> Result<Value> {
        let project = input.project_id;
        if input.title.trim().is_empty()
            || input.title.len() > 200
            || input.title.chars().any(char::is_control)
        {
            return Err(device_invalid());
        }
        let sha = hash(json_string(&("android-thread-fork/1", &input))?.as_bytes());
        let key = input
            .idempotency_key
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        operation_key(&key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.identity()
            .validate_principal_in_transaction(&mut tx, actor)
            .await
            .map_err(identity_error)?;
        let current = device_writer(&mut tx).await?;
        if let Some(old) =
            original(self, &mut tx, actor, project, &key, "thread_fork", &sha).await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(old);
        }
        let p = scope(self, &mut tx, actor, project, ContentAction::Write).await?;
        let parent = if let Some(id) = input.forked_from_thread_id {
            id
        } else {
            let message = input.forked_from_message_id.ok_or_else(device_invalid)?;
            let personal:Option<String>=sqlx::query_scalar("SELECT thread_id FROM android_personal_messages WHERE id=$1 AND project_id=$2 AND deleted=0").bind(message.to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            let copied:Option<String>=sqlx::query_scalar("SELECT thread_id FROM android_project_thread_history WHERE id=$1 AND project_id=$2").bind(message.to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            let origin = Uuid::from_u128(message.as_u128() ^ 1);
            let run:Option<String>=sqlx::query_scalar("SELECT order_id FROM android_scoped_runs WHERE (id=$1 OR id=$2) AND project_id=$3 UNION ALL SELECT thread_id FROM android_private_server_runs WHERE (id=$1 OR id=$2) AND project_id=$3 UNION ALL SELECT order_id FROM team_runs WHERE (id=$1 OR id=$2) AND project_id=$3 LIMIT 1").bind(message.to_string()).bind(origin.to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            Uuid::parse_str(&personal.or(copied).or(run).ok_or(Error::NotFound)?)
                .map_err(storage_error)?
        };
        source_thread(&mut tx, &p, parent).await?;
        let mut messages = thread_messages(&mut tx, actor, &p, parent).await?;
        if let Some(cut) = input.forked_from_message_id {
            let index = messages
                .iter()
                .position(|v| v["id"] == json!(cut))
                .ok_or(Error::Conflict)?;
            messages.truncate(index + 1);
        }
        let id = Uuid::new_v4();
        let value = json!({"schema_version":2,"id":id,"revision":1,"etag":format!("{id}:1"),"project_id":project,"forked_from_thread_id":parent,"forked_from_message_id":input.forked_from_message_id,"title":input.title.trim(),"deleted_at":null,"created_at":device_iso(current)?});
        if let Some(team) = p.team {
            let order = TeamOrder {
                id,
                team_id: team,
                project_id: project,
                title: input.title.trim().into(),
                prompt: String::new(),
                revision: 1,
                archived: false,
                created_at: current,
            };
            let payload = json_string(&order)?;
            sqlx::query("INSERT INTO team_orders(id,team_id,project_id,creator_id,creation_key,request_sha,initial_receipt,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$7)").bind(id.to_string()).bind(team.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).bind(format!("fork:{key}")).bind(&sha).bind(payload).execute(&mut *tx).await.map_err(storage_error)?;
        } else {
            sqlx::query("INSERT INTO android_personal_threads(id,project_id,owner_id,revision,payload,deleted,creation_key,request_sha256) VALUES($1,$2,$3,1,$4,0,$5,$6)").bind(id.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).bind(json_string(&value)?).bind(format!("fork:{key}")).bind(&sha).execute(&mut *tx).await.map_err(storage_error)?;
        }
        sqlx::query("INSERT INTO android_project_thread_lineage(thread_id,project_id,creator_id,source_thread_id,source_message_id,receipt,receipt_sha256) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id.to_string()).bind(project.to_string()).bind(actor.identity_id().to_string()).bind(parent.to_string()).bind(input.forked_from_message_id.map(|v|v.to_string())).bind(json_string(&value)?).bind(hash(json_string(&value)?.as_bytes())).execute(&mut *tx).await.map_err(storage_error)?;
        for mut message in messages {
            let source = crate::android_personal::wire_uuid(&message, "id")?;
            let original_sha = hash(json_string(&message)?.as_bytes());
            let copied = Uuid::new_v4();
            message["id"] = json!(copied);
            message["thread_id"] = json!(id);
            message["run_id"] = Value::Null;
            message["author_user_id"] = Value::Null;
            message["revision"] = json!(1);
            message["etag"] = json!(format!("{copied}:1"));
            message["created_at"] = json!(device_iso(current)?);
            message["updated_at"] = json!(device_iso(current)?);
            message["provenance"] = json!({"format":"android-thread-fork-message/1","source_thread_id":parent,"source_message_id":source,"original_sha256":original_sha,"inert_history":true});
            if p.team.is_none() {
                sqlx::query("INSERT INTO android_personal_messages(id,owner_id,project_id,thread_id,revision,payload,deleted) VALUES($1,$2,$3,$4,1,$5,0)").bind(copied.to_string()).bind(actor.identity_id().to_string()).bind(project.to_string()).bind(id.to_string()).bind(json_string(&message)?).execute(&mut *tx).await.map_err(storage_error)?;
            } else {
                sqlx::query("INSERT INTO android_project_thread_history(id,project_id,thread_id,team_id,creator_id,payload,source_id,original_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(copied.to_string()).bind(project.to_string()).bind(id.to_string()).bind(p.team.map(|v|v.to_string())).bind(actor.identity_id().to_string()).bind(json_string(&message)?).bind(source.to_string()).bind(original_sha).execute(&mut *tx).await.map_err(storage_error)?;
            }
            crate::android_wire_sync::record(
                &mut tx,
                actor.identity_id(),
                "message",
                copied,
                p.team,
                Some(project),
                1,
                &message,
                false,
                current,
            )
            .await?;
        }
        crate::android_wire_sync::record(
            &mut tx,
            actor.identity_id(),
            "thread",
            id,
            p.team,
            Some(project),
            1,
            &value,
            false,
            current,
        )
        .await?;
        receipt(
            &mut tx,
            actor.identity_id(),
            project,
            &key,
            "thread_fork",
            &sha,
            &value,
        )
        .await?;
        scope(self, &mut tx, actor, project, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn android_project_copied_messages(
        &self,
        actor: &Principal,
        project: Uuid,
        thread: Uuid,
    ) -> Result<Vec<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let p = scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        source_thread(&mut tx, &p, thread).await?;
        let mut after = String::new();
        let mut size = 0u64;
        let mut values = Vec::new();
        loop {
            let row = sqlx::query("SELECT id,CASE WHEN LENGTH(payload)<=16777216 THEN payload ELSE NULL END AS payload FROM android_project_thread_history WHERE project_id=$1 AND thread_id=$2 AND id>$3 ORDER BY id LIMIT 1").bind(project.to_string()).bind(thread.to_string()).bind(&after).fetch_optional(&mut *tx).await.map_err(storage_error)?;
            let Some(row) = row else { break };
            after = row.try_get("id").map_err(storage_error)?;
            let encoded = row
                .try_get::<Option<String>, _>("payload")
                .map_err(storage_error)?
                .ok_or(Error::BudgetExceeded)?;
            size = size
                .checked_add(encoded.len() as u64)
                .ok_or(Error::BudgetExceeded)?;
            if size > content::MAX_CONTENT || values.len() >= 10000 {
                return Err(Error::BudgetExceeded);
            }
            values.push(parse(&encoded)?);
        }
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub async fn android_project_thread_lineage(
        &self,
        actor: &Principal,
        project: Uuid,
        thread: Uuid,
    ) -> Result<Option<Value>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let p = scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        let current = source_thread(&mut tx, &p, thread).await?;
        let row=sqlx::query("SELECT source_thread_id,source_message_id,receipt,receipt_sha256 FROM android_project_thread_lineage WHERE thread_id=$1 AND project_id=$2").bind(thread.to_string()).bind(project.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let Some(row) = row else {
            scope(self, &mut tx, actor, project, ContentAction::Read).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        let bytes: String = row.try_get("receipt").map_err(storage_error)?;
        if hash(bytes.as_bytes())
            != row
                .try_get::<String, _>("receipt_sha256")
                .map_err(storage_error)?
        {
            return Err(Error::Conflict);
        }
        let original: Value = parse(&bytes)?;
        if original["id"] != json!(thread) || original["project_id"] != json!(project) {
            return Err(Error::Conflict);
        }
        let revision = current["revision"].as_i64().ok_or(Error::Conflict)?;
        let value = json!({"schema_version":2,"id":thread,"project_id":project,"revision":revision,"etag":format!("{thread}:{revision}"),"title":current["title"],"deleted_at":current.get("deleted_at").cloned().unwrap_or(Value::Null),"created_at":original["created_at"],"forked_from_thread_id":row.try_get::<String,_>("source_thread_id").map_err(storage_error)?,"forked_from_message_id":row.try_get::<Option<String>,_>("source_message_id").map_err(storage_error)?});
        scope(self, &mut tx, actor, project, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Some(value))
    }
}
