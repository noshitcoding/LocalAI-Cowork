use super::*;
struct Captured {
    plan: TeamLostHostPlan,
    raw: Vec<u8>,
}
fn original(row: &AnyRow, file: &TeamLostHostRecord) -> Result<Value> {
    if file.raw.len() > 4096 {
        return Err(invalid());
    }
    let actual: Value = serde_json::from_slice(&file.raw).map_err(|_| invalid())?;
    let mut source: Value = parse(
        &row.try_get::<String, _>("source_record")
            .map_err(storage_error)?,
    )?;
    if digest(&source)?
        != row
            .try_get::<String, _>("source_sha256")
            .map_err(storage_error)?
        || uuid_value(&actual["nonce"])? != file.nonce
        || actual["format"] != 2
        || actual["name"] != format!("cowork-team-{}", file.nonce)
        || row.try_get::<String, _>("nonce").map_err(storage_error)? != file.nonce.to_string()
    {
        return Err(Error::Conflict);
    }
    if let Some(cid) = actual["container_id"].as_str() {
        if !sha(cid) {
            return Err(invalid());
        }
        source["container_id"] = Value::String(cid.to_owned());
    }
    if source != actual {
        return Err(Error::Conflict);
    }
    if let Some(received) = row
        .try_get::<Option<String>, _>("received_record_sha256")
        .map_err(storage_error)?
    {
        if received != bytes_sha(&file.raw)
            || row
                .try_get::<Option<String>, _>("received_container_id")
                .map_err(storage_error)?
                .as_deref()
                != actual["container_id"].as_str()
        {
            return Err(Error::Conflict);
        }
    }
    let reference: TeamHostAdmissionRef =
        serde_json::from_value(actual["trusted_host_admission"].clone())
            .map_err(|_| Error::UncertainEffect)?;
    if reference.profile_id.to_string()
        != row
            .try_get::<String, _>("profile_id")
            .map_err(storage_error)?
        || reference.profile_revision
            != row
                .try_get::<i64, _>("profile_revision")
                .map_err(storage_error)?
        || reference.profile_sha256
            != row
                .try_get::<String, _>("profile_sha256")
                .map_err(storage_error)?
    {
        return Err(Error::Conflict);
    }
    Ok(actual)
}
async fn scope_members(
    service: &TeamService,
    tx: &mut Transaction<'_, Any>,
    actor: &Principal,
    p: &OperatorHostProfile,
) -> Result<Vec<AnyRow>> {
    let rows=sqlx::query("SELECT a.* FROM team_container_host_admissions a JOIN team_host_trust_profiles p ON p.id=a.profile_id WHERE p.host_id=$1 AND p.engine_id=$2 AND p.vm_id=$3 AND a.retired_receipt_sha256 IS NULL AND a.normal_removed_receipt IS NULL ORDER BY a.nonce LIMIT 10001").bind(p.binding.host_id.to_string()).bind(&p.binding.engine_id).bind(p.vm_id.to_string()).fetch_all(&mut**tx).await.map_err(storage_error)?;
    if rows.is_empty() || rows.len() > 10_000 {
        return Err(Error::BudgetExceeded);
    }
    let mut runs = BTreeSet::new();
    for row in &rows {
        if row.try_get::<String, _>("team_id").map_err(storage_error)? != p.team_id.to_string() {
            return Err(Error::Forbidden);
        }
        let run = Uuid::parse_str(&row.try_get::<String, _>("run_id").map_err(storage_error)?)
            .map_err(|_| invalid())?;
        if runs.insert(run) {
            service
                .container_recovery_authority(tx, actor, p.team_id, run)
                .await?;
        }
    }
    Ok(rows)
}
fn intent_for(
    p: &OperatorHostProfile,
    operation: Uuid,
    selected: &Captured,
    rows: &[AnyRow],
) -> Result<Value> {
    let mut runtime = BTreeSet::new();
    let mut manifest = Sha256::new();
    for row in rows {
        let source: Value = parse(
            &row.try_get::<String, _>("source_record")
                .map_err(storage_error)?,
        )?;
        let name = source["name"].as_str().ok_or_else(invalid)?;
        runtime.insert(name.to_owned());
        if let Some(cid) = row
            .try_get::<Option<String>, _>("received_container_id")
            .map_err(storage_error)?
        {
            runtime.insert(cid);
        }
        let tuple = serde_json::json!({"nonce":row.try_get::<String,_>("nonce").map_err(storage_error)?,"run_id":row.try_get::<String,_>("run_id").map_err(storage_error)?,"order_id":row.try_get::<String,_>("order_id").map_err(storage_error)?,"approval_id":row.try_get::<String,_>("approval_id").map_err(storage_error)?,"approval_request_sha256":row.try_get::<String,_>("approval_request_sha256").map_err(storage_error)?,"original_lease_owner":row.try_get::<String,_>("original_lease_owner").map_err(storage_error)?,"original_binding_sha256":bytes_sha(row.try_get::<String,_>("original_binding").map_err(storage_error)?.as_bytes()),"original_source_sha256":row.try_get::<String,_>("source_sha256").map_err(storage_error)?,"received_record_sha256":row.try_get::<Option<String>,_>("received_record_sha256").map_err(storage_error)?,"dispatch_admitted":row.try_get::<i64,_>("dispatch_admitted").map_err(storage_error)?});
        let raw = serde_json::to_vec(&tuple).map_err(storage_error)?;
        manifest.update((raw.len() as u64).to_be_bytes());
        manifest.update(raw);
    }
    if runtime.is_empty() || runtime.len() > 10_000 {
        return Err(Error::BudgetExceeded);
    }
    let out = serde_json::json!({"format":"cowork-original-host-retirement-intent-v1","request_id":operation,"operation_id":operation,"organisation_id":p.manager_organisation_id,"tenant_team_id":p.team_id,"instance_id":p.instance_id,"host_id":p.binding.host_id,"vm_id":p.vm_id,"operator_state_id":p.operator_state_id,"execution_epoch":p.execution_epoch,"daemon_id":p.daemon_id,"minimum_isolation":"virtual_machine","original_runtime_ids":runtime,"engine_id":p.binding.engine_id,"node_name":p.binding.node_name,"node_uid":p.binding.node_uid,"selected_run_id":selected.plan.run_id,"selected_order_id":selected.plan.order_id,"selected_nonce":selected.plan.nonce,"selected_container_id":selected.plan.container_id,"selected_container_name":selected.plan.container_name,"selected_record_sha256":selected.plan.original_record_sha256,"original_profile_sha256":digest(p)?,"original_allocation_sha256":p.allocation_sha256,"original_backend_activation_sha256":digest(&p.backend_activation)?,"original_manager_identity_sha256":digest(&p.original_runtime_identity)?,"original_manager_identity_signature_sha256":digest(&p.original_runtime_signature)?,"original_admission_count":rows.len(),"original_admission_manifest_sha256":format!("{:x}",manifest.finalize())});
    if serde_json::to_vec(&out).map_err(storage_error)?.len() > 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    Ok(out)
}
fn preview_sha(p: &OperatorHostProfile, c: &Captured, rows: &[AnyRow]) -> Result<String> {
    digest(
        &serde_json::json!({"nonce":c.plan.nonce,"original_record_sha256":c.plan.original_record_sha256,"profile_sha256":digest(p)?,"profile_revision":c.plan.current_revision,"intent":intent_for(p,Uuid::nil(),c,rows)?}),
    )
}
impl TeamService {
    async fn host_retirement_admin(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        team: Uuid,
    ) -> Result<()> {
        self.store
            .identity()
            .organization_in_transaction(tx, actor, team)
            .await
            .map_err(identity_error)?;
        let role:Option<String>=sqlx::query_scalar("SELECT role FROM identity_memberships WHERE identity_id=$1 AND organization_id=$2 AND active=1").bind(actor.identity_id().to_string()).bind(team.to_string()).fetch_optional(&mut**tx).await.map_err(storage_error)?;
        if !role
            .as_deref()
            .is_some_and(|v| matches!(v, "owner" | "admin"))
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    async fn capture_lost(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
    ) -> Result<Vec<Captured>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let value = self
            .container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        tx.commit().await.map_err(storage_error)?;
        let mut records = self
            .executor
            .lost_host_records(team, value.order_id, run)
            .await?;
        if records.len() > 64 {
            return Err(Error::BudgetExceeded);
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        let known: BTreeSet<Uuid> = records.iter().map(|r| r.nonce).collect();
        let journal=sqlx::query("SELECT nonce,source_record,received_record FROM team_container_host_admissions WHERE run_id=$1 AND team_id=$2 AND profile_id IS NOT NULL AND normal_removed_receipt IS NULL AND retired_receipt_sha256 IS NULL ORDER BY nonce LIMIT 65").bind(run.to_string()).bind(team.to_string()).fetch_all(&mut*tx).await.map_err(storage_error)?;
        if journal.len() > 64 {
            return Err(Error::BudgetExceeded);
        }
        for row in journal {
            let nonce = Uuid::parse_str(&row.try_get::<String, _>("nonce").map_err(storage_error)?)
                .map_err(|_| invalid())?;
            if !known.contains(&nonce) {
                let raw = match row
                    .try_get::<Option<String>, _>("received_record")
                    .map_err(storage_error)?
                {
                    Some(raw) => crypto::base64(&raw, 4096)?,
                    None => row
                        .try_get::<String, _>("source_record")
                        .map_err(storage_error)?
                        .into_bytes(),
                };
                records.push(TeamLostHostRecord {
                    nonce,
                    raw,
                    original_source: "original_host_admission",
                });
            }
        }
        let mut result = Vec::new();
        for file in records {
            let Some(row)=sqlx::query("SELECT * FROM team_container_host_admissions WHERE nonce=$1 AND run_id=$2 AND team_id=$3").bind(file.nonce.to_string()).bind(run.to_string()).bind(team.to_string()).fetch_optional(&mut*tx).await.map_err(storage_error)? else{continue;};
            if row
                .try_get::<Option<String>, _>("retired_receipt_sha256")
                .map_err(storage_error)?
                .is_some()
            {
                continue;
            }
            if row
                .try_get::<Option<String>, _>("profile_id")
                .map_err(storage_error)?
                .is_none()
            {
                continue;
            }
            let actual = original(&row, &file)?;
            let profile = sqlx::query("SELECT * FROM team_host_trust_profiles WHERE id=$1")
                .bind(
                    row.try_get::<String, _>("profile_id")
                        .map_err(storage_error)?,
                )
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
            let p = registry::profile(&profile)?;
            if actual["host"] != serde_json::to_value(&p.binding).map_err(storage_error)?
                || p.team_id != team
                || digest(&p)?
                    != row
                        .try_get::<String, _>("profile_sha256")
                        .map_err(storage_error)?
            {
                return Err(Error::Conflict);
            }
            result.push(Captured {
                plan: TeamLostHostPlan {
                    run_id: run,
                    order_id: value.order_id,
                    team_id: team,
                    nonce: file.nonce,
                    container_id: actual["container_id"].as_str().map(str::to_owned),
                    container_name: actual["name"].as_str().ok_or_else(invalid)?.to_owned(),
                    original_record_sha256: bytes_sha(&file.raw),
                    original_record_source: file.original_source.to_owned(),
                    profile_id: p.id,
                    original_profile_sha256: digest(&p)?,
                    intent: Value::Null,
                    intent_sha256: String::new(),
                    current_revision: profile.try_get("revision").map_err(storage_error)?,
                    plan_sha256: String::new(),
                    affected_runs: Vec::new(),
                    original_runtime_count: 0,
                },
                raw: file.raw,
            });
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    async fn lost_host_plans_selected(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        selected: Option<Uuid>,
    ) -> Result<Vec<TeamLostHostPlan>> {
        let captured = self.capture_lost(actor, team, run).await?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for c in captured {
            if selected.is_some_and(|nonce| nonce != c.plan.nonce)
                || !seen.insert(c.plan.profile_id)
            {
                continue;
            }
            let row = sqlx::query("SELECT * FROM team_host_trust_profiles WHERE id=$1")
                .bind(c.plan.profile_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
            let p = registry::profile(&row)?;
            let rows = scope_members(self, &mut tx, actor, &p).await?;
            let mut plan = c.plan.clone();
            plan.intent = intent_for(&p, Uuid::nil(), &c, &rows)?;
            plan.plan_sha256 = preview_sha(&p, &c, &rows)?;
            plan.affected_runs = rows
                .iter()
                .map(|r| {
                    r.try_get::<String, _>("run_id")
                        .map_err(storage_error)
                        .and_then(|v| Uuid::parse_str(&v).map_err(|_| invalid()))
                })
                .collect::<Result<BTreeSet<_>>>()?
                .into_iter()
                .collect();
            plan.original_runtime_count = u32::try_from(rows.len()).map_err(|_| invalid())?;
            if selected.is_none() {
                plan.intent = serde_json::json!({"format":"cowork-original-host-retirement-summary-v1","vm_id":p.vm_id,"host_id":p.binding.host_id,"engine_id":p.binding.engine_id,"node_uid":p.binding.node_uid,"public_key_sha256":p.public_key_sha256,"original_admission_count":rows.len(),"complete_original_plan_available":true});
            }
            out.push(plan);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(out)
    }
    pub async fn lost_host_plans(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
    ) -> Result<Vec<TeamLostHostPlan>> {
        self.lost_host_plans_selected(actor, team, run, None).await
    }
    pub async fn lost_host_plan(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        nonce: Uuid,
    ) -> Result<TeamLostHostPlan> {
        self.lost_host_plans_selected(actor, team, run, Some(nonce))
            .await?
            .into_iter()
            .next()
            .ok_or(Error::NotFound)
    }
    pub async fn prepare_lost_host(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        input: PrepareTeamLostHostRetirement,
    ) -> Result<TeamLostHostPlan> {
        if input.operation_id.is_nil()
            || input.nonce.is_nil()
            || input.expected_revision < 1
            || !sha(&input.original_plan_sha256)
        {
            return Err(invalid());
        }
        let request_sha = digest(&input)?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        self.host_retirement_admin(&mut tx, actor, team).await?;
        if let Some(old)=sqlx::query("SELECT actor_id,request_sha256,original_intent FROM team_lost_host_retirement_operations WHERE run_id=$1 AND operation_id=$2").bind(run.to_string()).bind(input.operation_id.to_string()).fetch_optional(&mut*tx).await.map_err(storage_error)?{if old.try_get::<String,_>("actor_id").map_err(storage_error)?!=actor.identity_id().to_string()||old.try_get::<String,_>("request_sha256").map_err(storage_error)?!=request_sha{return Err(Error::Conflict);}let p:TeamLostHostPlan=parse(&old.try_get::<String,_>("original_intent").map_err(storage_error)?)?;tx.commit().await.map_err(storage_error)?;return Ok(p);}
        tx.commit().await.map_err(storage_error)?;
        let mut selected = self
            .capture_lost(actor, team, run)
            .await?
            .into_iter()
            .find(|c| c.plan.nonce == input.nonce)
            .ok_or(Error::NotFound)?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        self.host_retirement_admin(&mut tx, actor, team).await?;
        let row = sqlx::query("SELECT * FROM team_host_trust_profiles WHERE id=$1")
            .bind(selected.plan.profile_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        let p = registry::profile(&row)?;
        let rows = scope_members(self, &mut tx, actor, &p).await?;
        if row.try_get::<i64, _>("revision").map_err(storage_error)? != input.expected_revision
            || row
                .try_get::<Option<i64>, _>("retired_at")
                .map_err(storage_error)?
                .is_some()
        {
            return Err(Error::Conflict);
        }
        let mut runs = BTreeSet::new();
        for r in &rows {
            runs.insert(
                Uuid::parse_str(&r.try_get::<String, _>("run_id").map_err(storage_error)?)
                    .map_err(|_| invalid())?,
            );
        }
        tx.commit().await.map_err(storage_error)?;
        let mut files = Vec::new();
        for affected in &runs {
            files.extend(
                self.capture_lost(actor, team, *affected)
                    .await?
                    .into_iter()
                    .filter(|c| c.plan.profile_id == p.id),
            );
        }
        if files.len() > 10_000 || files.iter().all(|c| c.plan.nonce != input.nonce) {
            return Err(Error::Conflict);
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        self.host_retirement_admin(&mut tx, actor, team).await?;
        registry::lock(&self.store, &mut tx).await?;
        let fresh = sqlx::query("SELECT * FROM team_host_trust_profiles WHERE id=$1")
            .bind(p.id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        if fresh.try_get::<i64, _>("revision").map_err(storage_error)? != input.expected_revision
            || digest(&registry::profile(&fresh)?)? != digest(&p)?
        {
            return Err(Error::Conflict);
        }
        let rows = scope_members(self, &mut tx, actor, &p).await?;
        if preview_sha(&p, &selected, &rows)? != input.original_plan_sha256 {
            return Err(Error::Conflict);
        }
        selected.plan.plan_sha256 = input.original_plan_sha256.clone();
        selected.plan.intent = intent_for(&p, input.operation_id, &selected, &rows)?;
        selected.plan.intent_sha256 = digest(&selected.plan.intent)?;
        selected.plan.affected_runs = runs.into_iter().collect();
        selected.plan.original_runtime_count = selected.plan.intent["original_runtime_ids"]
            .as_array()
            .ok_or_else(invalid)?
            .len()
            .try_into()
            .map_err(|_| invalid())?;
        let clock = db_now(&self.store, &mut tx).await?;
        sqlx::query("INSERT INTO team_lost_host_retirement_operations(run_id,operation_id,actor_id,nonce,request_sha256,intent_sha256,original_intent,original_record_sha256,original_profile_sha256,state,original_created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'admitted',$10)").bind(run.to_string()).bind(input.operation_id.to_string()).bind(actor.identity_id().to_string()).bind(input.nonce.to_string()).bind(&request_sha).bind(&selected.plan.intent_sha256).bind(json_string(&selected.plan)?).bind(&selected.plan.original_record_sha256).bind(&selected.plan.original_profile_sha256).bind(clock).execute(&mut*tx).await.map_err(storage_error)?;
        for file in files {
            let original_row = rows
                .iter()
                .find(|r| {
                    r.try_get::<String, _>("nonce").ok().as_deref()
                        == Some(file.plan.nonce.to_string().as_str())
                })
                .ok_or(Error::Conflict)?;
            original(
                original_row,
                &TeamLostHostRecord {
                    nonce: file.plan.nonce,
                    raw: file.raw.clone(),
                    original_source: "original_host_admission",
                },
            )?;
            sqlx::query("INSERT INTO team_lost_host_retirement_members(operation_run_id,operation_id,nonce,run_id,order_id,source_record,source_record_sha256) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(run.to_string()).bind(input.operation_id.to_string()).bind(file.plan.nonce.to_string()).bind(file.plan.run_id.to_string()).bind(file.plan.order_id.to_string()).bind(STANDARD.encode(&file.raw)).bind(&file.plan.original_record_sha256).execute(&mut*tx).await.map_err(storage_error)?;
        }
        // Global writer lock + exact host tuple prevents another admission between the manifest and maintenance fence.
        sqlx::query("UPDATE team_host_trust_profiles SET active=0,revision=revision+1 WHERE host_id=$1 AND engine_id=$2 AND vm_id=$3").bind(p.binding.host_id.to_string()).bind(&p.binding.engine_id).bind(p.vm_id.to_string()).execute(&mut*tx).await.map_err(storage_error)?;
        self.container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        self.host_retirement_admin(&mut tx, actor, team).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(selected.plan)
    }
    pub async fn lost_host_operation(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        operation: Uuid,
    ) -> Result<Option<Value>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.recovery_access(&mut tx, actor, team, run, false)
            .await?;
        self.recovery_owner(&mut tx, actor, team, run).await?;
        let r=sqlx::query("SELECT actor_id,request_sha256,original_intent,state,receipt FROM team_lost_host_retirement_operations WHERE run_id=$1 AND operation_id=$2").bind(run.to_string()).bind(operation.to_string()).fetch_optional(&mut*tx).await.map_err(storage_error)?;
        let result=r.map(|r|{let p:TeamLostHostPlan=parse(&r.try_get::<String,_>("original_intent").map_err(storage_error)?)?;let receipt:Option<Value>=r.try_get::<Option<String>,_>("receipt").map_err(storage_error)?.as_deref().map(parse).transpose()?;Ok::<_,Error>(serde_json::json!({"plan":p,"state":r.try_get::<String,_>("state").map_err(storage_error)?,"request_sha256":r.try_get::<String,_>("request_sha256").map_err(storage_error)?,"receipt":receipt}))}).transpose()?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn receive_lost_host(
        &self,
        actor: &Principal,
        team: Uuid,
        run: Uuid,
        input: ReceiveTeamLostHostRetirement,
    ) -> Result<TeamLostHostRetirementReceipt> {
        if input.operation_id.is_nil()
            || !sha(&input.original_intent_sha256)
            || serde_json::to_vec(&input.witness)
                .map_err(storage_error)?
                .len()
                > 2 * 1024 * 1024
        {
            return Err(invalid());
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        self.host_retirement_admin(&mut tx, actor, team).await?;
        let row=sqlx::query("SELECT * FROM team_lost_host_retirement_operations WHERE run_id=$1 AND operation_id=$2").bind(run.to_string()).bind(input.operation_id.to_string()).fetch_one(&mut*tx).await.map_err(storage_error)?;
        let plan: TeamLostHostPlan = parse(
            &row.try_get::<String, _>("original_intent")
                .map_err(storage_error)?,
        )?;
        if plan.team_id != team
            || plan.run_id != run
            || row
                .try_get::<String, _>("intent_sha256")
                .map_err(storage_error)?
                != input.original_intent_sha256
            || digest(&plan.intent)? != input.original_intent_sha256
        {
            return Err(Error::Conflict);
        }
        let p = registry::profile(
            &sqlx::query("SELECT * FROM team_host_trust_profiles WHERE id=$1")
                .bind(plan.profile_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?,
        )?;
        if digest(&p)? != plan.original_profile_sha256 {
            return Err(Error::Conflict);
        }
        let request_sha = row
            .try_get::<String, _>("request_sha256")
            .map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        let retirement = crypto::verify(&p, &plan.intent, &input.witness)?;
        let witness_sha = digest(&input.witness)?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        registry::lock(&self.store, &mut tx).await?;
        let old=sqlx::query("SELECT * FROM team_lost_host_retirement_operations WHERE run_id=$1 AND operation_id=$2").bind(run.to_string()).bind(input.operation_id.to_string()).fetch_one(&mut*tx).await.map_err(storage_error)?;
        if old
            .try_get::<String, _>("original_intent")
            .map_err(storage_error)?
            != json_string(&plan)?
            || old
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != request_sha
        {
            return Err(Error::Conflict);
        }
        let receipt = TeamLostHostRetirementReceipt {
            operation_id: input.operation_id,
            run_id: run,
            nonce: plan.nonce,
            original_record_sha256: plan.original_record_sha256.clone(),
            original_profile_sha256: plan.original_profile_sha256.clone(),
            original_intent_sha256: plan.intent_sha256.clone(),
            witness_sha256: witness_sha.clone(),
            retirement_id: retirement,
            host_retired: true,
            container_removed: false,
            effects_unknown: true,
            workspace_promotion_allowed: false,
        };
        if let Some(prior) = old
            .try_get::<Option<String>, _>("receipt")
            .map_err(storage_error)?
        {
            if prior != json_string(&receipt)?
                || old
                    .try_get::<Option<String>, _>("witness_sha256")
                    .map_err(storage_error)?
                    .as_deref()
                    != Some(witness_sha.as_str())
            {
                return Err(Error::Conflict);
            }
        } else {
            let changed=sqlx::query("UPDATE team_lost_host_retirement_operations SET state='received',witness=$1,witness_sha256=$2,receipt=$3 WHERE run_id=$4 AND operation_id=$5 AND state='admitted' AND witness IS NULL").bind(json_string(&input.witness)?).bind(&witness_sha).bind(json_string(&receipt)?).bind(run.to_string()).bind(input.operation_id.to_string()).execute(&mut*tx).await.map_err(storage_error)?;
            if changed.rows_affected() != 1 {
                return Err(Error::Conflict);
            }
            sqlx::query("UPDATE team_host_trust_profiles SET active=0,retired_at=$1 WHERE host_id=$2 AND engine_id=$3 AND vm_id=$4 AND retired_at IS NULL").bind(db_now(&self.store,&mut tx).await?).bind(p.binding.host_id.to_string()).bind(&p.binding.engine_id).bind(p.vm_id.to_string()).execute(&mut*tx).await.map_err(storage_error)?;
        }
        let members=sqlx::query("SELECT * FROM team_lost_host_retirement_members WHERE operation_run_id=$1 AND operation_id=$2 ORDER BY nonce LIMIT 10001").bind(run.to_string()).bind(input.operation_id.to_string()).fetch_all(&mut*tx).await.map_err(storage_error)?;
        if members.is_empty() || members.len() > 10_000 {
            return Err(Error::Conflict);
        }
        let mut permits = Vec::new();
        for m in members {
            let raw = crypto::base64(
                &m.try_get::<String, _>("source_record")
                    .map_err(storage_error)?,
                4096,
            )?;
            if bytes_sha(&raw)
                != m.try_get::<String, _>("source_record_sha256")
                    .map_err(storage_error)?
            {
                return Err(Error::Conflict);
            }
            let actual: Value = serde_json::from_slice(&raw).map_err(|_| invalid())?;
            let original_ref: TeamHostAdmissionRef =
                serde_json::from_value(actual["trusted_host_admission"].clone())
                    .map_err(|_| Error::Conflict)?;
            if original_ref.profile_id != p.id || original_ref.profile_sha256 != digest(&p)? {
                return Err(Error::Conflict);
            }
            let nonce = uuid_value(&actual["nonce"])?;
            let member_run = uuid_value(&actual["run"])?;
            let mut member_receipt = receipt.clone();
            member_receipt.run_id = member_run;
            member_receipt.nonce = nonce;
            member_receipt.original_record_sha256 = bytes_sha(&raw);
            let mut member_plan = plan.clone();
            member_plan.run_id = member_run;
            member_plan.order_id = uuid_value(&actual["order"])?;
            member_plan.nonce = nonce;
            member_plan.container_name = actual["name"].as_str().ok_or_else(invalid)?.into();
            member_plan.container_id = actual["container_id"].as_str().map(str::to_owned);
            member_plan.original_record_sha256 = bytes_sha(&raw);
            member_plan.original_record_source = "original_host_admission".into();
            if let Some(existing) = m
                .try_get::<Option<String>, _>("closure_receipt")
                .map_err(storage_error)?
            {
                if existing != json_string(&member_receipt)? {
                    return Err(Error::Conflict);
                }
            } else {
                sqlx::query("UPDATE team_lost_host_retirement_members SET closure_receipt=$1 WHERE operation_run_id=$2 AND operation_id=$3 AND nonce=$4 AND closure_receipt IS NULL").bind(json_string(&member_receipt)?).bind(run.to_string()).bind(input.operation_id.to_string()).bind(nonce.to_string()).execute(&mut*tx).await.map_err(storage_error)?;
            }
            sqlx::query("UPDATE team_container_host_admissions SET retired_receipt_sha256=$1 WHERE nonce=$2 AND retired_receipt_sha256 IS NULL").bind(digest(&member_receipt)?).bind(nonce.to_string()).execute(&mut*tx).await.map_err(storage_error)?;
            if m.try_get::<i64, _>("closed").map_err(storage_error)? == 0 {
                permits.push(LostHostClosurePermit {
                    service: self.clone(),
                    actor: actor.clone(),
                    plan: member_plan,
                    receipt: member_receipt,
                    source: raw,
                    request_sha256: request_sha.clone(),
                    operation_run_id: run,
                });
            }
        }
        tx.commit().await.map_err(storage_error)?;
        // The original observation is durable before current authority/IO; lost response never repeats retirement.
        for permit in permits {
            permit.revalidate().await?;
            let nonce = permit.plan.nonce;
            self.executor.close_lost_host_record(permit.clone()).await?;
            let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
            permit.revalidate_in(&mut tx).await?;
            sqlx::query("UPDATE team_lost_host_retirement_members SET closed=1 WHERE operation_run_id=$1 AND operation_id=$2 AND nonce=$3 AND closure_receipt=$4").bind(run.to_string()).bind(input.operation_id.to_string()).bind(nonce.to_string()).bind(json_string(&permit.receipt)?).execute(&mut*tx).await.map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
        }
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.container_recovery_authority(&mut tx, actor, team, run)
            .await?;
        self.host_retirement_admin(&mut tx, actor, team).await?;
        sqlx::query("UPDATE team_lost_host_retirement_operations SET state='closed' WHERE run_id=$1 AND operation_id=$2 AND state='received' AND NOT EXISTS(SELECT 1 FROM team_lost_host_retirement_members m WHERE m.operation_run_id=$1 AND m.operation_id=$2 AND m.closed=0)").bind(run.to_string()).bind(input.operation_id.to_string()).execute(&mut*tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(receipt)
    }
}
impl LostHostClosurePermit {
    async fn revalidate_in(&self, tx: &mut Transaction<'_, Any>) -> Result<()> {
        self.service
            .container_recovery_authority(tx, &self.actor, self.plan.team_id, self.plan.run_id)
            .await?;
        self.service
            .host_retirement_admin(tx, &self.actor, self.plan.team_id)
            .await?;
        let source:Option<String>=sqlx::query_scalar("SELECT closure_receipt FROM team_lost_host_retirement_members WHERE operation_run_id=$1 AND operation_id=$2 AND nonce=$3 AND source_record_sha256=$4").bind(self.operation_run_id.to_string()).bind(self.receipt.operation_id.to_string()).bind(self.plan.nonce.to_string()).bind(bytes_sha(&self.source)).fetch_one(&mut**tx).await.map_err(storage_error)?;
        if source.as_deref() != Some(json_string(&self.receipt)?.as_str()) {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

impl LostHostClosurePermit {
    /// Called only after acquiring the actual filesystem/workspace guards. Bounded
    /// original archive IO executes within current Identity/Run/Originalmember locks.
    pub async fn close_original_io(
        &self,
        operation: impl FnOnce() -> Result<()> + Send,
    ) -> Result<()> {
        let mut tx = self
            .service
            .store
            .pool
            .begin()
            .await
            .map_err(storage_error)?;
        self.revalidate_in(&mut tx).await?;
        self.service
            .container_recovery_authority(&mut tx, &self.actor, self.plan.team_id, self.plan.run_id)
            .await?;
        operation()?;
        tx.commit().await.map_err(storage_error)
    }
}
