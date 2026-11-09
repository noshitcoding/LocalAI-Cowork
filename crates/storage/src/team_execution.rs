//! One transactional resolver shared by Team review and existing run admission.
//! No run insertion, credential opening, model dispatch or tool effect occurs here.
use super::*;
use cowork_workflow_contracts::{
    TeamExecutionBlocker, TeamExecutionDocumentBinding, TeamExecutionMemoryBinding,
    TeamExecutionModelBinding, TeamExecutionPreflight, TeamExecutionRequest,
};

pub(super) struct ResolvedTeamExecution {
    pub(super) run: TeamRun,
    pub(super) binding: ExecutionBinding,
    pub(super) context: String,
    pub(super) preflight: TeamExecutionPreflight,
}

impl TeamService {
    /// Resolves authenticated review facts without creating a run. The common
    /// target/snapshot adapter must bind actual execution authority separately.
    pub async fn preflight_execution(
        &self,
        actor: &Principal,
        input: &TeamExecutionRequest,
    ) -> Result<TeamExecutionPreflight> {
        if input.request.idempotency_key.starts_with("team-schedule:") {
            return Err(Error::Forbidden);
        }
        self.validate_team_execution_request(&input.request)?;
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let (org, order, permit) = self
            .team_execution_scope_in_transaction(
                &mut tx,
                actor,
                input.team_id,
                &input.request,
                None,
            )
            .await?;
        let resolved = self
            .resolve_team_execution_in_transaction(
                &mut tx,
                actor,
                &org,
                &order,
                &input.request,
                None,
                None,
            )
            .await?;
        self.store
            .identity()
            .validate_content_in_transaction(&mut tx, &permit)
            .await
            .map_err(identity_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(resolved.preflight)
    }

    pub(super) fn validate_team_execution_request(&self, input: &CreateTeamRun) -> Result<()> {
        safe_text(&input.prompt, 128 * 1024)?;
        key(&input.idempotency_key)?;
        validate_plan(&input.plan)?;
        if !self.executor.supports(&input.plan) {
            return Err(Error::Validation(
                "Workflow or tools are not supported by the configured team executor".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn team_execution_scope_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        team: Uuid,
        input: &CreateTeamRun,
        reserved: Option<(Uuid, i64, i64)>,
    ) -> Result<(Organization, TeamOrder, cowork_identity::ContentPermit)> {
        let identity = self.store.identity();
        let org = identity
            .organization_in_transaction(&mut *tx, actor, team)
            .await
            .map_err(identity_error)?;
        let order = self.order_in(&mut *tx, team, input.order_id).await?;
        if let Some((_, order_revision, _)) = reserved
            && order.revision != order_revision
        {
            return Err(Error::Forbidden);
        }
        if order.archived {
            return Err(Error::Conflict);
        }
        let permit = self
            .project_permit(
                &mut *tx,
                actor,
                team,
                order.project_id,
                ContentAction::Execute,
            )
            .await?;
        self.project_permit(&mut *tx, actor, team, order.project_id, ContentAction::Read)
            .await?;
        Ok((org, order, permit))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "resolve_team_execution_in_transaction: Keep actor/organization/order, confirmed run request and reserved project/order/provider revisions explicit"
    )]
    pub(super) async fn resolve_team_execution_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        org: &Organization,
        order: &TeamOrder,
        input: &CreateTeamRun,
        expected_project_revision: Option<i64>,
        reserved: Option<(Uuid, i64, i64)>,
    ) -> Result<ResolvedTeamExecution> {
        let team = org.id;
        let project_revision: i64 =
            sqlx::query_scalar("SELECT revision FROM team_project_profiles WHERE project_id=$1")
                .bind(order.project_id.to_string())
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?
                .unwrap_or(1);
        if expected_project_revision.is_some_and(|expected| project_revision != expected) {
            return Err(Error::Conflict);
        }
        let p = provider(
            &sqlx::query("SELECT * FROM team_provider WHERE team_id=$1")
                .bind(team.to_string())
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage_error)?
                .ok_or(Error::Validation(
                    "Configure the team's LiteLLM provider first".into(),
                ))?,
        )?;
        if let Some((_, _, provider_revision)) = reserved
            && p.revision != provider_revision
        {
            return Err(Error::Forbidden);
        }
        let actor_binding = self
            .store
            .identity()
            .resolve_team_execution_actor_in_transaction(tx, actor, team, order.project_id)
            .await
            .map_err(identity_error)?;
        let mut selected: BTreeMap<Uuid, TeamDocumentKind> = BTreeMap::new();
        for id in &input.plan.skills {
            selected.insert(*id, TeamDocumentKind::Skill);
        }
        for id in &input.plan.knowledge {
            if selected.insert(*id, TeamDocumentKind::Knowledge).is_some() {
                return Err(invalid());
            }
        }
        if let Some(w) = &input.plan.workflow {
            for agent in &w.agents {
                for id in &agent.skills {
                    if selected
                        .get(id)
                        .is_some_and(|kind| kind != &TeamDocumentKind::Skill)
                    {
                        return Err(invalid());
                    }
                    selected.insert(*id, TeamDocumentKind::Skill);
                }
                for id in &agent.knowledge {
                    if selected
                        .get(id)
                        .is_some_and(|kind| kind != &TeamDocumentKind::Knowledge)
                    {
                        return Err(invalid());
                    }
                    selected.insert(*id, TeamDocumentKind::Knowledge);
                }
            }
        }
        if selected
            .values()
            .any(|kind| kind == &TeamDocumentKind::Knowledge)
            && !crate::knowledge_access::team_allowed(tx, order.project_id).await?
        {
            return Err(Error::Validation("knowledge_base_disabled".into()));
        }
        let mut documents = Vec::new();
        for (id, kind) in selected {
            let document:TeamDocument=parse(&sqlx::query("SELECT payload FROM team_documents WHERE id=$1 AND team_id=$2 AND project_id=$3").bind(id.to_string()).bind(team.to_string()).bind(order.project_id.to_string()).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?.try_get::<String,_>("payload").map_err(storage_error)?)?;
            if document.archived
                || document.kind != kind
                || document.id != id
                || document.team_id != team
                || document.project_id != order.project_id
            {
                return Err(Error::Conflict);
            }
            documents.push(document);
        }
        // The creator's personal knowledge base reaches results the team reads, so it
        // has its own administrator switch, independent of team project knowledge.
        let personal = if input.plan.personal_knowledge {
            if !crate::knowledge_access::personal_team_allowed(tx, actor.identity_id()).await? {
                return Err(Error::Validation("knowledge_base_disabled".into()));
            }
            crate::knowledge_access::personal_team_knowledge(
                tx,
                actor.identity_id(),
                &format!("{}\n{}", order.prompt, input.prompt),
            )
            .await?
        } else {
            Vec::new()
        };
        let run = TeamRun {
            output: final_team_output(&input.plan).cloned(),
            id: reserved.map(|(id, _, _)| id).unwrap_or_else(Uuid::new_v4),
            team_id: team,
            project_id: order.project_id,
            order_id: order.id,
            status: TeamRunStatus::Queued,
            provider_revision: p.revision,
            model: p.model.clone(),
            response: None,
            error: None,
            created_at: now(),
            updated_at: now(),
        };
        let (mcp_servers, github_revision) =
            integrations::freeze_tools(&mut *tx, team, &input.plan.tools).await?;
        let binding = ExecutionBinding {
            run_id: run.id,
            team_id: team,
            project_id: order.project_id,
            order_id: order.id,
            identity_id: actor.identity_id(),
            session_id: actor.session_id(),
            session_revision: actor_binding.session_revision,
            organization_epoch: actor_binding.organization_epoch,
            membership_id: actor_binding.membership_id.to_string(),
            membership_revision: actor_binding.membership_revision,
            resource_revision: actor_binding.resource_revision,
            grant_revision: actor_binding.grant_revision,
            provider_revision: p.revision,
            order_revision: order.revision,
            documents: documents.iter().map(|d| (d.id, d.revision)).collect(),
            mcp_servers,
            github_revision,
            personal_knowledge: personal.iter().map(|e| (e.id, e.revision)).collect(),
        };
        validate_binding(&self.store, &mut *tx, &binding, "").await?;
        let preflight = TeamExecutionPreflight {
            // The existing TeamExecutor trait has no qualified common target
            // binding. Resolution alone must never claim execution readiness.
            ready: false,
            blockers: vec![TeamExecutionBlocker::UnboundExecutionTarget],
            actor: actor_binding,
            request: input.clone(),
            project_revision,
            order_revision: order.revision,
            order_prompt: order.prompt.clone(),
            model: TeamExecutionModelBinding {
                provider_revision: p.revision,
                base_url: p.base_url,
                model: p.model,
            },
            documents: documents
                .iter()
                .map(|d| TeamExecutionDocumentBinding {
                    id: d.id,
                    kind: d.kind.clone(),
                    name: d.name.clone(),
                    revision: d.revision,
                    content_sha256: hash(d.content.as_bytes()),
                })
                .collect(),
            personal_knowledge: personal
                .iter()
                .map(|e| TeamExecutionMemoryBinding {
                    id: e.id,
                    revision: e.revision,
                })
                .collect(),
            mcp_servers: binding
                .mcp_servers
                .iter()
                .map(|(id, b)| b.execution_review(*id))
                .collect(),
            github_revision: binding.github_revision,
        };
        let context = if personal.is_empty() {
            json_string(&(&order.prompt, &documents))?
        } else {
            json_string(&(&order.prompt, &documents, &personal))?
        };
        Ok(ResolvedTeamExecution {
            run,
            binding,
            context,
            preflight,
        })
    }
}
