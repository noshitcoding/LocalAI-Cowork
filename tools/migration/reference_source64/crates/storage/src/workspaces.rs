use super::*;

impl Store {
    pub async fn workspace(&self, order: &str) -> Result<Option<Workspace>> {
        let payload: Option<String> =
            sqlx::query_scalar("SELECT payload FROM workspaces WHERE order_id=$1")
                .bind(order)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        payload.map(|p| parse(&p)).transpose()
    }

    pub async fn register_workspace(&self, workspace: &Workspace) -> Result<()> {
        let order = self.order(&workspace.order_id).await?;
        if workspace.folder.order_id.as_deref() != Some(&order.id)
            || workspace.folder.project_id != order.project_id
        {
            return Err(Error::Forbidden);
        }
        sqlx::query("INSERT INTO workspaces(order_id,payload) VALUES($1,$2) ON CONFLICT(order_id) DO NOTHING")
            .bind(&order.id).bind(json_string(workspace)?).execute(&self.pool).await.map_err(storage_error)?;
        if self.workspace(&order.id).await?.as_ref() != Some(workspace) {
            return Err(Error::Conflict);
        }
        Ok(())
    }

    pub async fn workspace_snapshot(&self, run: &str) -> Result<Option<WorkspaceSnapshot>> {
        let payload: Option<String> =
            sqlx::query_scalar("SELECT payload FROM workspace_snapshots WHERE run_id=$1")
                .bind(run)
                .fetch_optional(&self.pool)
                .await
                .map_err(storage_error)?;
        let snapshot: Option<WorkspaceSnapshot> = payload.map(|p| parse(&p)).transpose()?;
        if let Some(s) = &snapshot {
            verify_workspace_snapshot(s)?;
        }
        Ok(snapshot)
    }

    pub async fn workspace_base(&self, order: &str) -> Result<Option<WorkspaceSnapshot>> {
        let history = self.history(order).await?;
        Ok(history.inherited.and_then(|s| {
            let run = s.messages.last().map(|m| m.run_id.clone())?;
            s.workspace_snapshots.into_iter().find(|w| w.run_id == run)
        }))
    }
}
