//! Immutable dispatch intentions and separate original ACKs. Admission is never
//! inferred from a missing result, and unknown effects cannot be rearmed.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
struct Intent {
    view: TeamEffectView,
    request: Value,
}
#[derive(Clone)]
pub(crate) struct EffectPermit {
    store: Store,
    view: TeamEffectView,
    owner: String,
    token: String,
}
pub(crate) enum EffectAdmission {
    Settled(Value),
    Dispatch(EffectPermit),
}
impl EffectPermit {
    pub(crate) fn request_id(&self) -> &str {
        &self.view.request_id
    }
    pub(crate) async fn acknowledge(&self, value: &Value) -> Result<()> {
        self.acknowledge_with_insights(value,None).await
    }
    pub(crate) async fn acknowledge_with_insights(&self,value:&Value,insights:Option<(Vec<AuditSkillUse>,u64,Option<Value>)>)->Result<()> {
        let raw = json_string(value)?;
        if raw.len() > 2 * 1024 * 1024 {
            return Err(invalid());
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE team_runs SET lease_until=lease_until WHERE id=$1")
            .bind(self.view.run_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let row=sqlx::query("SELECT payload,permit_token,dispatch_owner FROM team_effect_intents WHERE id=$1 AND run_id=$2").bind(self.view.id.to_string()).bind(self.view.run_id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let intent: Intent = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if intent.view.request_sha256 != self.view.request_sha256
            || intent.view.scope_sha256 != self.view.scope_sha256
            || row
                .try_get::<String, _>("permit_token")
                .map_err(storage_error)?
                != self.token
            || row
                .try_get::<String, _>("dispatch_owner")
                .map_err(storage_error)?
                != self.owner
        {
            return Err(Error::Forbidden);
        }
        if let Some(old) =
            sqlx::query("SELECT outcome,result FROM team_effect_receipts WHERE effect_id=$1")
                .bind(self.view.id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
        {
            if old.try_get::<String, _>("outcome").map_err(storage_error)? != "completed"
                || old
                    .try_get::<Option<String>, _>("result")
                    .map_err(storage_error)?
                    .as_deref()
                    != Some(raw.as_str())
            {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(());
        }
        if intent.view.state != "admitted" {
            return Err(Error::Conflict);
        }
        // The permit proves the original dispatch, not current authority. Recording
        // a real response after revocation preserves facts without advancing work.
        let mut receipt = json!({"effect_id":self.view.id,"run_id":self.view.run_id,"request_id":self.view.request_id,"request_sha256":self.view.request_sha256,"scope_sha256":self.view.scope_sha256,"kind":self.view.kind,"outcome":"completed","result":value});
        if let Some((skills,duration_ms,metadata))=insights{receipt["insights"]=json!({"skills":skills,"duration_ms":duration_ms,"observed_at":now()});if let Some(metadata)=metadata{receipt["insights"]["received_cost_metadata"]=metadata;}}
        sqlx::query("INSERT INTO team_effect_receipts(effect_id,outcome,source,payload,result) VALUES($1,'completed','original_worker_ack',$2,$3)").bind(self.view.id.to_string()).bind(json_string(&receipt)?).bind(raw).execute(&mut *tx).await.map_err(storage_error)?;
        let team:String=sqlx::query_scalar("SELECT team_id FROM team_runs WHERE id=$1").bind(self.view.run_id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        cowork_identity::audit_insights::team_receipt(&mut tx,Uuid::parse_str(&team).map_err(storage_error)?,self.view.run_id,&serde_json::to_value(&self.view).map_err(storage_error)?,&receipt,now()).await.map_err(storage_error)?;
        let mut intent = intent;
        intent.view.state = "completed".into();
        sqlx::query("UPDATE team_effect_intents SET state='completed',payload=$1 WHERE id=$2")
            .bind(json_string(&intent)?)
            .bind(self.view.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::query("UPDATE team_recovery_runs SET revision=revision+1 WHERE run_id=$1")
            .bind(self.view.run_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)
    }
}
async fn scope_sha(
    store: &Store,
    tx: &mut Transaction<'_, Any>,
    binding: &ExecutionBinding,
) -> Result<String> {
    let row = sqlx::query("SELECT request_sha,context FROM team_runs WHERE id=$1")
        .bind(binding.run_id.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let _ = store;
    Ok(hash(json_string(&json!({"run":binding.run_id,"team":binding.team_id,"project":binding.project_id,"order":binding.order_id,"provider_revision":binding.provider_revision,"order_revision":binding.order_revision,"documents":binding.documents,"mcp_servers":binding.mcp_servers,"github_revision":binding.github_revision,"request_sha":row.try_get::<String,_>("request_sha").map_err(storage_error)?,"context_sha":hash(row.try_get::<String,_>("context").map_err(storage_error)?.as_bytes())}))?.as_bytes()))
}
pub(crate) async fn initialize(
    tx: &mut Transaction<'_, Any>,
    store: &Store,
    binding: &ExecutionBinding,
    enabled: bool,
) -> Result<()> {
    let scope = scope_sha(store, tx, binding).await?;
    sqlx::query("INSERT INTO team_recovery_runs(run_id,revision,scope_sha256,journal_version) VALUES($1,1,$2,$3)").bind(binding.run_id.to_string()).bind(scope).bind(i64::from(enabled)).execute(&mut **tx).await.map_err(storage_error)?;
    Ok(())
}
impl TeamExecutionAuthority {
    pub(crate) async fn admit_effect(
        &self,
        slot: &str,
        kind: &str,
        request: Value,
    ) -> Result<EffectAdmission> {
        key(slot)?;
        if !["model", "tool"].contains(&kind) || json_string(&request)?.len() > 2 * 1024 * 1024 {
            return Err(invalid());
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
        let scope = scope_sha(&self.store, &mut tx, &self.binding).await?;
        let request_sha = hash(json_string(&(scope.as_str(), kind, &request))?.as_bytes());
        let previous=sqlx::query("SELECT payload FROM team_effect_intents WHERE run_id=$1 AND slot=$2 ORDER BY attempt DESC LIMIT 1").bind(self.run_id().to_string()).bind(slot).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let mut attempt = 1;
        if let Some(row) = previous {
            let old: Intent = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            if old.view.request_sha256 != request_sha
                || old.view.scope_sha256 != scope
                || old.view.kind != kind
            {
                return Err(Error::Conflict);
            }
            match old.view.state.as_str() {
                "completed" => {
                    let result:String=sqlx::query_scalar("SELECT result FROM team_effect_receipts WHERE effect_id=$1 AND outcome='completed'").bind(old.view.id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
                    tx.commit().await.map_err(storage_error)?;
                    return Ok(EffectAdmission::Settled(parse(&result)?));
                }
                "not_executed" => {
                    attempt = old.view.attempt.checked_add(1).ok_or(Error::Conflict)?
                }
                _ => return Err(Error::UncertainEffect),
            }
        }
        let revision:i64=sqlx::query_scalar("SELECT revision FROM team_recovery_runs WHERE run_id=$1 AND scope_sha256=$2 AND journal_version=1").bind(self.run_id().to_string()).bind(&scope).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let view = TeamEffectView {
            id: Uuid::new_v4(),
            run_id: self.run_id(),
            slot: slot.into(),
            attempt,
            sequence: revision.checked_add(1).ok_or(Error::Conflict)?,
            kind: kind.into(),
            state: "admitted".into(),
            request_sha256: request_sha,
            request_id: Uuid::new_v4().to_string(),
            scope_sha256: scope,
            provider_revision: self.binding.provider_revision,
            created_at: db_now(&self.store, &mut tx).await?,
        };
        let token = Uuid::new_v4().to_string();
        let intent = Intent {
            view: view.clone(),
            request,
        };
        sqlx::query("INSERT INTO team_effect_intents(id,run_id,slot,attempt,sequence,kind,state,request_sha256,scope_sha256,dispatch_owner,permit_token,payload) VALUES($1,$2,$3,$4,$5,$6,'admitted',$7,$8,$9,$10,$11)").bind(view.id.to_string()).bind(view.run_id.to_string()).bind(slot).bind(attempt).bind(view.sequence).bind(kind).bind(&view.request_sha256).bind(&view.scope_sha256).bind(&self.lease_owner).bind(&token).bind(json_string(&intent)?).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE team_recovery_runs SET revision=revision+1 WHERE run_id=$1")
            .bind(view.run_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let state = TeamRunStep {
            id: slot.into(),
            state: "started".into(),
            response: None,
            error: None,
            updated_at: now(),
        };
        sqlx::query("INSERT INTO team_run_steps(run_id,step_id,payload) VALUES($1,$2,$3) ON CONFLICT(run_id,step_id) DO UPDATE SET payload=excluded.payload").bind(view.run_id.to_string()).bind(slot).bind(json_string(&state)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(EffectAdmission::Dispatch(EffectPermit {
            store: self.store.clone(),
            view,
            owner: self.lease_owner.clone(),
            token,
        }))
    }
    pub(crate) fn with_effect(&self, permit: &EffectPermit) -> Self {
        let mut next = self.clone();
        next.effect = Some(permit.clone());
        next
    }
    pub async fn record_tool_result(&self, value: &Value) -> Result<()> {
        if let Some(permit) = &self.effect {
            if permit.view.kind != "tool" {
                return Err(Error::Forbidden);
            }
            permit.acknowledge(value).await?;
        }
        Ok(())
    }
    pub(crate) async fn checkpoints(&self) -> Result<BTreeMap<String, String>> {
        self.revalidate().await?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let scope = scope_sha(&self.store, &mut tx, &self.binding).await?;
        let rows = sqlx::query(
            "SELECT step_id,response,scope_sha256 FROM team_graph_checkpoints WHERE run_id=$1",
        )
        .bind(self.run_id().to_string())
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?;
        let mut values = BTreeMap::new();
        for row in rows {
            if row
                .try_get::<String, _>("scope_sha256")
                .map_err(storage_error)?
                != scope
            {
                return Err(Error::Conflict);
            }
            values.insert(
                row.try_get("step_id").map_err(storage_error)?,
                row.try_get("response").map_err(storage_error)?,
            );
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(values)
    }
    pub(crate) async fn checkpoint(&self, step: &str, response: &str) -> Result<()> {
        safe_text(response, 2 * 1024 * 1024)?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        validate_binding(&self.store, &mut tx, &self.binding, &self.lease_owner).await?;
        let scope = scope_sha(&self.store, &mut tx, &self.binding).await?;
        if let Some(old) = sqlx::query_scalar::<_, String>(
            "SELECT response FROM team_graph_checkpoints WHERE run_id=$1 AND step_id=$2",
        )
        .bind(self.run_id().to_string())
        .bind(step)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        {
            if old != response {
                return Err(Error::Conflict);
            }
        } else {
            sqlx::query("INSERT INTO team_graph_checkpoints(run_id,step_id,scope_sha256,response) VALUES($1,$2,$3,$4)").bind(self.run_id().to_string()).bind(step).bind(scope).bind(response).execute(&mut *tx).await.map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)
    }
}
impl TeamService {
    async fn recovery_owner(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
    ) -> Result<()> {
        let creator: String =
            sqlx::query_scalar("SELECT creator_id FROM team_runs WHERE id=$1 AND team_id=$2")
                .bind(run.to_string())
                .bind(team.to_string())
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
        let role:String=sqlx::query_scalar("SELECT role FROM identity_memberships WHERE identity_id=$1 AND organization_id=$2 AND active=1").bind(actor.identity_id().to_string()).bind(team.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        if creator != actor.identity_id().to_string() && !matches!(role.as_str(), "owner" | "admin")
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    async fn recovery_binding(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        team: Uuid,
        run: &TeamRun,
        mut binding: ExecutionBinding,
    ) -> Result<ExecutionBinding> {
        self.recovery_owner(tx, actor, team, run.id).await?;
        let permit = self
            .project_permit(tx, actor, team, run.project_id, ContentAction::Execute)
            .await?;
        self.store
            .identity()
            .validate_content_in_transaction(tx, &permit)
            .await
            .map_err(identity_error)?;
        let org = self
            .store
            .identity()
            .organization_in_transaction(tx, actor, team)
            .await
            .map_err(identity_error)?;
        let a=sqlx::query("SELECT s.revision AS session_revision,m.id AS membership_id,m.revision AS membership_revision,r.revision AS resource_revision,g.revision AS grant_revision FROM identity_sessions s JOIN identity_memberships m ON m.identity_id=s.identity_id AND m.organization_id=$2 JOIN identity_resources r ON r.id=$3 JOIN identity_content_grants g ON g.resource_id=r.id AND g.membership_id=m.id WHERE s.id=$1 AND m.active=1 AND g.active=1").bind(actor.session_id().to_string()).bind(team.to_string()).bind(run.project_id.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        binding.identity_id = actor.identity_id();
        binding.session_id = actor.session_id();
        binding.session_revision = a.try_get("session_revision").map_err(storage_error)?;
        binding.membership_id = a.try_get("membership_id").map_err(storage_error)?;
        binding.membership_revision = a.try_get("membership_revision").map_err(storage_error)?;
        binding.organization_epoch = org.authorization_epoch;
        binding.resource_revision = a.try_get("resource_revision").map_err(storage_error)?;
        binding.grant_revision = a.try_get("grant_revision").map_err(storage_error)?;
        validate_binding(&self.store, tx, &binding, "").await?;
        let row=sqlx::query("SELECT * FROM team_provider WHERE team_id=$1").bind(team.to_string()).fetch_one(&mut **tx).await.map_err(storage_error)?;
        let provider=provider(&row)?;
        if provider.revision!=run.provider_revision||provider.model!=run.model||validate_provider(&provider.base_url,&provider.model,&self.allowed_http_origins)?!=provider.base_url{return Err(Error::Conflict);}
        let sealed:SealedCredential=parse(&row.try_get::<String,_>("sealed_key").map_err(storage_error)?)?;
        if sealed.key_id()!=self.key_id{return Err(Error::NativeKeyUnavailable);}
        let _credential=open_bearer(self.keys.as_ref(),&scope(&provider),&sealed).map_err(|_|Error::NativeKeyUnavailable)?;
        Ok(binding)
    }
    async fn recovery_access(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        team: Uuid,
        run_id: Uuid,
        write: bool,
    ) -> Result<(TeamRun, ExecutionBinding)> {
        let row = sqlx::query("SELECT * FROM team_runs WHERE id=$1 AND team_id=$2")
            .bind(run_id.to_string())
            .bind(team.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .ok_or(Error::NotFound)?;
        let run = decode_run(&row)?;
        let permit = self
            .project_permit(
                tx,
                actor,
                team,
                run.project_id,
                if write {
                    ContentAction::Execute
                } else {
                    ContentAction::Read
                },
            )
            .await?;
        self.store
            .identity()
            .validate_content_in_transaction(tx, &permit)
            .await
            .map_err(identity_error)?;
        if write {
            self.recovery_owner(tx, actor, team, run_id).await?;
            let clock = db_now(&self.store, tx).await?;
            if !matches!(
                run.status,
                TeamRunStatus::Unknown | TeamRunStatus::Cancelled
            ) || row
                .try_get::<Option<String>, _>("lease_owner")
                .map_err(storage_error)?
                .is_some()
                || row
                    .try_get::<i64, _>("lease_until")
                    .map_err(storage_error)?
                    > clock
            {
                return Err(Error::Conflict);
            }
        }
        Ok((
            run,
            parse(&row.try_get::<String, _>("binding").map_err(storage_error)?)?,
        ))
    }
    pub async fn recovery(
        &self,
        actor: &Principal,
        team: Uuid,
        run_id: Uuid,
        after: i64,
    ) -> Result<TeamRecoveryView> {
        if after < 0 {
            return Err(invalid());
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let (run, binding) = self
            .recovery_access(&mut tx, actor, team, run_id, false)
            .await?;
        let state = sqlx::query(
            "SELECT revision,scope_sha256,journal_version FROM team_recovery_runs WHERE run_id=$1",
        )
        .bind(run_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        let (revision, scope, version) = if let Some(row) = state {
            (
                row.try_get::<i64, _>("revision").map_err(storage_error)?,
                row.try_get::<String, _>("scope_sha256")
                    .map_err(storage_error)?,
                row.try_get::<i64, _>("journal_version")
                    .map_err(storage_error)?,
            )
        } else {
            (0, scope_sha(&self.store, &mut tx, &binding).await?, 0)
        };
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM team_effect_intents WHERE run_id=$1 AND state='admitted'",
        )
        .bind(run_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let legacy:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_run_steps s WHERE s.run_id=$1 AND NOT EXISTS(SELECT 1 FROM team_effect_intents i WHERE i.run_id=s.run_id AND i.slot=s.step_id)").bind(run_id.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let reason = if version != 1 || !self.executor.supports_recovery() {
            Some("original_journal_unavailable")
        } else if legacy > 0 {
            Some("legacy_step_receipts_incomplete")
        } else if !matches!(
            run.status,
            TeamRunStatus::Unknown | TeamRunStatus::Cancelled
        ) || self
            .recovery_access(&mut tx, actor, team, run_id, true)
            .await
            .is_err()
        {
            Some("run_not_quiescent_or_recovery_not_authorized")
        } else if pending > 0 {
            Some("unconfirmed_original_effects")
        } else if scope != scope_sha(&self.store, &mut tx, &binding).await?
            || self
                .recovery_binding(&mut tx, actor, team, &run, binding.clone())
                .await
                .is_err()
        {
            Some("current_authority_or_frozen_plan_changed")
        } else {
            None
        };
        let rows=sqlx::query("SELECT payload FROM team_effect_intents WHERE run_id=$1 AND sequence>$2 ORDER BY sequence LIMIT 501").bind(run_id.to_string()).bind(after).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let has_more = rows.len() > 500;
        let effects = rows
            .into_iter()
            .take(500)
            .map(|r| {
                parse::<Intent>(&r.try_get::<String, _>("payload").map_err(storage_error)?)
                    .map(|i| i.view)
            })
            .collect::<Result<Vec<_>>>()?;
        let cursor = effects.last().map_or(after, |e| e.sequence);
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamRecoveryView {
            run_id,
            revision,
            scope_sha256: scope,
            status: run.status,
            resume_allowed: reason.is_none(),
            blocked_reason: reason.map(str::to_owned),
            effects,
            next_cursor: cursor,
            has_more,
        })
    }
    pub async fn effect_detail(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        effect: Uuid,
    ) -> Result<TeamEffectDetail> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.recovery_access(&mut tx, actor, team, run, false)
            .await?;
        let payload: String =
            sqlx::query_scalar("SELECT payload FROM team_effect_intents WHERE run_id=$1 AND id=$2")
                .bind(run.to_string())
                .bind(effect.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
        let intent: Intent = parse(&payload)?;
        let row = sqlx::query(
            "SELECT outcome,source,result,payload FROM team_effect_receipts WHERE effect_id=$1",
        )
        .bind(effect.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        let (outcome, source, result, receipt) = if let Some(row) = row {
            (
                Some(row.try_get::<String, _>("outcome").map_err(storage_error)?),
                Some(row.try_get::<String, _>("source").map_err(storage_error)?),
                row.try_get::<Option<String>, _>("result")
                    .map_err(storage_error)?
                    .map(|s| parse(&s))
                    .transpose()?,
                Some(parse(
                    &row.try_get::<String, _>("payload").map_err(storage_error)?,
                )?),
            )
        } else {
            (None, None, None, None)
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(TeamEffectDetail {
            effect: intent.view,
            request: intent.request,
            outcome,
            source,
            result,
            receipt,
        })
    }
    async fn recovery_operation(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        run: Uuid,
        key: Uuid,
        digest: &str,
    ) -> Result<Option<Value>> {
        let row=sqlx::query("SELECT request_sha256,receipt FROM team_recovery_operations WHERE run_id=$1 AND actor_id=$2 AND idempotency_key=$3").bind(run.to_string()).bind(actor.identity_id().to_string()).bind(key.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?;
        row.map(|r| {
            if r.try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != digest
            {
                return Err(Error::Conflict);
            }
            parse(&r.try_get::<String, _>("receipt").map_err(storage_error)?)
        })
        .transpose()
    }
    async fn save_recovery_operation(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        run: Uuid,
        key: Uuid,
        digest: &str,
        receipt: &Value,
    ) -> Result<()> {
        sqlx::query("INSERT INTO team_recovery_operations(run_id,actor_id,idempotency_key,request_sha256,receipt) VALUES($1,$2,$3,$4,$5)").bind(run.to_string()).bind(actor.identity_id().to_string()).bind(key.to_string()).bind(digest).bind(json_string(receipt)?).execute(&mut **tx).await.map_err(storage_error)?;
        Ok(())
    }
    pub async fn recovery_receipt(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        key: Uuid,
    ) -> Result<Value> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.recovery_access(&mut tx, actor, team, run, false)
            .await?;
        let payload:String=sqlx::query_scalar("SELECT receipt FROM team_recovery_operations WHERE run_id=$1 AND actor_id=$2 AND idempotency_key=$3").bind(run.to_string()).bind(actor.identity_id().to_string()).bind(key.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
        let value = parse(&payload)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
    pub async fn resolve_effect(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        input: ResolveTeamEffect,
    ) -> Result<Value> {
        if !input.owner_verified
            || !matches!(input.outcome.as_str(), "completed" | "not_executed")
            || input.source.trim().len() < 8
            || input.source.len() > 2048
            || json_string(&input.receipt)?.len() > 2 * 1024 * 1024
        {
            return Err(invalid());
        }
        let digest = hash(json_string(&("resolve", team, run, &input))?.as_bytes());
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE team_runs SET lease_until=lease_until WHERE id=$1")
            .bind(run.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        self.recovery_access(&mut tx, actor, team, run, false)
            .await?;
        if let Some(receipt) = self
            .recovery_operation(&mut tx, actor, run, input.idempotency_key, &digest)
            .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        self.recovery_access(&mut tx, actor, team, run, true)
            .await?;
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM team_recovery_runs WHERE run_id=$1 AND journal_version=1",
        )
        .bind(run.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::Forbidden)?;
        if revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        let payload: String =
            sqlx::query_scalar("SELECT payload FROM team_effect_intents WHERE id=$1 AND run_id=$2")
                .bind(input.effect_id.to_string())
                .bind(run.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::NotFound)?;
        let mut intent: Intent = parse(&payload)?;
        let e = &intent.view;
        if e.state != "admitted"
            || e.request_sha256 != input.request_sha256
            || input.receipt["effect_id"] != json!(e.id)
            || input.receipt["run_id"] != json!(run)
            || input.receipt["request_id"] != e.request_id
            || input.receipt["request_sha256"] != e.request_sha256
            || input.receipt["scope_sha256"] != e.scope_sha256
            || input.receipt["kind"] != e.kind
            || input.receipt["outcome"] != input.outcome
        {
            return Err(Error::Conflict);
        }
        let result = if input.outcome == "completed" {
            let value = input.receipt.get("result").ok_or_else(invalid)?;
            if e.kind == "model"
                && value
                    .pointer("/choices/0/message")
                    .is_none_or(|m| !m.is_object())
            {
                return Err(invalid());
            }
            Some(json_string(value)?)
        } else {
            if input.receipt["execution_state"] != "not_started"
                || input.receipt.get("result").is_some()
            {
                return Err(invalid());
            }
            None
        };
        sqlx::query("INSERT INTO team_effect_receipts(effect_id,outcome,source,payload,result) VALUES($1,$2,$3,$4,$5)").bind(e.id.to_string()).bind(&input.outcome).bind(&input.source).bind(json_string(&input.receipt)?).bind(result).execute(&mut *tx).await.map_err(storage_error)?;
        intent.view.state = input.outcome;
        sqlx::query("UPDATE team_effect_intents SET state=$1,payload=$2 WHERE id=$3")
            .bind(&intent.view.state)
            .bind(json_string(&intent)?)
            .bind(intent.view.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        sqlx::query("UPDATE team_recovery_runs SET revision=revision+1 WHERE run_id=$1")
            .bind(run.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let receipt =
            json!({"effect_id":intent.view.id,"outcome":intent.view.state,"revision":revision+1});
        self.save_recovery_operation(
            &mut tx,
            actor,
            run,
            input.idempotency_key,
            &digest,
            &receipt,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
    pub async fn resume_recovered(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        input: ResumeTeamRun,
    ) -> Result<Value> {
        let digest = hash(json_string(&("resume", team, run, &input))?.as_bytes());
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        sqlx::query("UPDATE team_runs SET lease_until=lease_until WHERE id=$1")
            .bind(run.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        self.recovery_access(&mut tx, actor, team, run, false)
            .await?;
        if let Some(receipt) = self
            .recovery_operation(&mut tx, actor, run, input.idempotency_key, &digest)
            .await?
        {
            tx.commit().await.map_err(storage_error)?;
            return Ok(receipt);
        }
        let (mut value, binding) = self
            .recovery_access(&mut tx, actor, team, run, true)
            .await?;
        if value.status == TeamRunStatus::Cancelled && !input.acknowledge_cancelled {
            return Err(Error::Forbidden);
        }
        if !self.executor.supports_recovery() {
            return Err(Error::Forbidden);
        }
        let row=sqlx::query("SELECT revision,scope_sha256 FROM team_recovery_runs WHERE run_id=$1 AND journal_version=1").bind(run.to_string()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::Forbidden)?;
        let revision: i64 = row.try_get("revision").map_err(storage_error)?;
        if revision != input.expected_revision
            || row
                .try_get::<String, _>("scope_sha256")
                .map_err(storage_error)?
                != scope_sha(&self.store, &mut tx, &binding).await?
        {
            return Err(Error::Conflict);
        }
        let unknown: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM team_effect_intents WHERE run_id=$1 AND state='admitted'",
        )
        .bind(run.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        let legacy:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_run_steps s WHERE s.run_id=$1 AND NOT EXISTS(SELECT 1 FROM team_effect_intents i WHERE i.run_id=s.run_id AND i.slot=s.step_id)").bind(run.to_string()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if unknown > 0 || legacy > 0 {
            return Err(Error::UncertainEffect);
        }
        let binding = self
            .recovery_binding(&mut tx, actor, team, &value, binding)
            .await?;
        value.status = TeamRunStatus::Queued;
        value.error = None;
        value.updated_at = now();
        sqlx::query("UPDATE team_runs SET status='queued',payload=$1,binding=$2,lease_owner=NULL,lease_until=0 WHERE id=$3").bind(json_string(&value)?).bind(json_string(&binding)?).bind(run.to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        sqlx::query("UPDATE team_recovery_runs SET revision=revision+1 WHERE run_id=$1")
            .bind(run.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let receipt = json!({"run":value,"revision":revision+1});
        self.save_recovery_operation(
            &mut tx,
            actor,
            run,
            input.idempotency_key,
            &digest,
            &receipt,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
