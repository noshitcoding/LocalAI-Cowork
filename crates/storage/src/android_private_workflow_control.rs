//! Compound graph decisions are committed with their exact original tool receipt.
use super::execution::{
    current_scope, node_record, rebuild_nodes, root_checkpoint, save_node_context_inner, save_root,
    scoped_authority, scoped_checkpoint,
};
use super::*;
use crate::android_private_server::{event, leased};
use crate::android_private_workspace as workspace;
use cowork_domain::workflows::*;
impl Store {
    pub async fn android_private_workflow_delegate(
        &self,
        run: Uuid,
        lease: Uuid,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<Value> {
        let key = workspace::writer_cipher(keys, key)?;
        let scope = current_scope().ok_or(Error::Forbidden)?;
        if scope.run_id() != run || scope.lease() != lease {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, run, lease, now).await?;
        let mut cp = scoped_checkpoint(&mut tx, &r, keys)
            .await?
            .ok_or(Error::Forbidden)?;
        scoped_authority(&mut tx, &r, &cp, now).await?;
        let tool = crate::android_private_tools::pending(&cp)?;
        if tool.name != "delegate_agents"
            || !cp["spec"]["allowed_tools"]
                .as_array()
                .is_some_and(|t| t.iter().any(|v| v == "delegate_agents"))
            || cp["pending_tools"].as_array().is_none_or(|v| v.len() != 1)
        {
            return Err(Error::Forbidden);
        }
        let before = root_checkpoint(&mut tx, &r, keys).await?;
        let plan: WorkflowPlan =
            serde_json::from_value(before["workflow"]["plan"].clone()).map_err(storage_error)?;
        let mut progress: WorkflowProgress =
            serde_json::from_value(before["workflow"]["progress"].clone())
                .map_err(storage_error)?;
        let requests: Vec<WorkflowDelegation> =
            serde_json::from_value(tool.arguments["requests"].clone())
                .map_err(|_| device_invalid())?;
        let invocation = crate::android_private_tools::invocation(&cp, &tool)?;
        let logical = format!("delegate-{}", &hash(invocation.as_bytes())[..40]);
        let children = progress.delegate(
            &plan,
            progress.revision(),
            &WorkflowClaim {
                node_id: scope.node_id().into(),
                token: scope.claim().into(),
            },
            &logical,
            requests,
            now / 1000,
        )?;
        for child in &children {
            let n = &progress.nodes().get(child).ok_or(Error::Forbidden)?.plan;
            sqlx::query("INSERT INTO android_private_workflow_nodes(run_id,node_id,role_id,state,revision) VALUES($1,$2,$3,'pending',1)").bind(run.to_string()).bind(child).bind(&n.role_id).execute(&mut *tx).await.map_err(storage_error)?;
        }
        let old = cp.clone();
        let sequence = crate::android_private_tools::next_sequence(&mut tx, run).await?;
        let result = json!({"format":"android-private-workflow-delegation/1","node_id":scope.node_id(),"children":children,"original_invocation":invocation,"wait_for_children":true});
        let intent = json!({"format":1,"tool":tool,"fingerprint":crate::android_private_tools::tool_fingerprint(run,&cp,&tool)});
        let receipt = json!({"format":1,"request_sha256":canonical_hash(&intent),"tool":tool,"result":result});
        let sealed_intent = workspace::seal_value(
            keys,
            key,
            &r,
            &format!("intent:{sequence}"),
            json_string(&intent)?.as_bytes(),
        )?;
        let sealed_receipt = workspace::seal_value(
            keys,
            key,
            &r,
            &format!("receipt:{sequence}"),
            json_string(&receipt)?.as_bytes(),
        )?;
        crate::server_keys::cipher_writer(&mut tx, key).await?;
        sqlx::query("INSERT INTO android_private_loop_steps(run_id,sequence,invocation_id,kind,lease_id,checkpoint_sha,request_sha,intent,receipt_sha,receipt,state,created_at) VALUES($1,$2,$3,'tool',$4,$5,$6,$7,$8,$9,'received',$10)").bind(run.to_string()).bind(invocation).bind(lease.to_string()).bind(canonical_hash(&old)).bind(canonical_hash(&intent)).bind(json_string(&sealed_intent)?).bind(canonical_hash(&receipt)).bind(json_string(&sealed_receipt)?).bind(now).execute(&mut *tx).await.map_err(storage_error)?;
        cp["pending_tools"]
            .as_array_mut()
            .ok_or(Error::Forbidden)?
            .clear();
        cp["workflow_yield"] = json!(true);
        cp["last_content"] = Value::Null;
        cp["tool_steps"] = json!(
            cp["tool_steps"]
                .as_u64()
                .ok_or(Error::Forbidden)?
                .checked_add(1)
                .ok_or(Error::BudgetExceeded)?
        );
        cp["messages"].as_array_mut().ok_or(Error::Forbidden)?.push(json!({"role":"tool","tool_call_id":tool.id,"name":tool.name,"content":json_string(&result)?}));
        save_node_context_inner(&mut tx, &r, &scope, &cp, &old, keys, key, now, false).await?;
        // This is a logical wait, not physical stop. Crew's original paused
        // guest must be removed by its runner before another parent claim.
        if sqlx::query("UPDATE android_private_workflow_nodes SET state='waiting',revision=revision+1 WHERE run_id=$1 AND node_id=$2 AND claim_token=$3 AND parent_lease_id=$4 AND state='running'").bind(run.to_string()).bind(scope.node_id()).bind(scope.claim()).bind(lease.to_string()).execute(&mut *tx).await.map_err(storage_error)?.rows_affected()!=1{return Err(Error::Conflict)}
        let mut after = before.clone();
        after["workflow"]["progress"] = serde_json::to_value(progress).map_err(storage_error)?;
        rebuild_nodes(&mut tx, &r, &mut after, keys).await?;
        save_root(&mut tx, &r, &before, &after, keys, key, now).await?;
        crate::android_private_tools::live_after_crypto(self, &mut tx, &r, &cp).await?;
        event(&mut tx,run,"workflow_delegated",&json!({"node_id":scope.node_id(),"children":result["children"],"original_receipt_sha256":canonical_hash(&receipt)}),now).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn android_private_workflow_fail_node(
        &self,
        scope: &PrivateWorkflowNodeScope,
        keys: &dyn KeyStore,
        key: KeyId,
    ) -> Result<bool> {
        let key = workspace::writer_cipher(keys, key)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let now = device_writer(&mut tx).await?;
        let r = leased(self, &mut tx, scope.run_id(), scope.lease(), now).await?;
        node_record(&mut tx, &r, scope).await?;
        let unknown:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_loop_steps WHERE run_id=$1 AND invocation_id LIKE $2 AND state!='received'").bind(scope.run_id().to_string()).bind(format!("workflow:{}:%",hash(scope.node_id().as_bytes()))).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let callbacks:i64=sqlx::query_scalar("SELECT COUNT(*) FROM android_private_workflow_rpcs WHERE run_id=$1 AND node_id=$2 AND state!='received'").bind(scope.run_id().to_string()).bind(scope.node_id()).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if unknown != 0 || callbacks != 0 {
            return Err(Error::UncertainEffect);
        }
        let before = root_checkpoint(&mut tx, &r, keys).await?;
        let plan: WorkflowPlan =
            serde_json::from_value(before["workflow"]["plan"].clone()).map_err(storage_error)?;
        let mut progress: WorkflowProgress =
            serde_json::from_value(before["workflow"]["progress"].clone())
                .map_err(storage_error)?;
        progress.fail(
            &plan,
            progress.revision(),
            &WorkflowClaim {
                node_id: scope.node_id().into(),
                token: scope.claim().into(),
            },
            "node_failed",
            now / 1000,
        )?;
        sqlx::query("UPDATE android_private_workflow_nodes SET state='failed',revision=revision+1 WHERE run_id=$1 AND node_id=$2 AND claim_token=$3 AND parent_lease_id=$4 AND state IN('running','waiting')").bind(scope.run_id().to_string()).bind(scope.node_id()).bind(scope.claim()).bind(scope.lease().to_string()).execute(&mut *tx).await.map_err(storage_error)?;
        for (node, p) in progress.nodes() {
            if p.state == WorkflowNodeState::Blocked {
                sqlx::query("UPDATE android_private_workflow_nodes SET state='blocked',revision=revision+1 WHERE run_id=$1 AND node_id=$2 AND state='pending'").bind(scope.run_id().to_string()).bind(node).execute(&mut *tx).await.map_err(storage_error)?;
            }
        }
        let mut after = before.clone();
        after["workflow"]["progress"] = serde_json::to_value(progress).map_err(storage_error)?;
        rebuild_nodes(&mut tx, &r, &mut after, keys).await?;
        save_root(&mut tx, &r, &before, &after, keys, key, now).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(plan.definition.failure == WorkflowFailure::ContinueIndependent)
    }
}
