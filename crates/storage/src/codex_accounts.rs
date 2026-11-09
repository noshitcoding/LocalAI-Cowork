use super::*;
use crate::library_config::{LibraryOwner, permit};
use cowork_identity::ContentAction;
impl Store {
    pub async fn original_codex_account_call(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        key: &str,
        revision: i64,
        operation: &str,
    ) -> Result<Option<Value>> {
        valid_text(key, 200)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        let row=sqlx::query("SELECT request_sha256,response FROM codex_account_calls WHERE scope=$1 AND operation_key=$2").bind(owner.namespace()).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        let result = if let Some(row) = row {
            if row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != canonical_hash(&json!((id, revision, operation)))
            {
                return Err(Error::Conflict);
            }
            let response: Option<String> = row.try_get("response").map_err(storage_error)?;
            Some(parse(&response.ok_or(Error::UncertainEffect)?)?)
        } else {
            None
        };
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
    pub async fn codex_account_has_actions(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM codex_account_calls WHERE scope=$1 AND account_id=$2",
        )
        .bind(owner.namespace())
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Read).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(count > 0)
    }
    pub async fn codex_accounts(&self, owner: LibraryOwner<'_>) -> Result<Vec<CodexAccount>> {
        self.library_list(owner, "codex_account").await
    }
    pub async fn codex_account(&self, owner: LibraryOwner<'_>, id: &str) -> Result<CodexAccount> {
        self.library_read(owner, "codex_account", id).await
    }
    pub async fn write_codex_account(
        &self,
        owner: LibraryOwner<'_>,
        id: Option<&str>,
        v: &WriteCodexAccount,
    ) -> Result<CodexAccount> {
        valid_text(&v.value.name, 200)?;
        if v.value.name.chars().any(char::is_control) || v.value.priority > 10000 {
            return Err(Error::Validation("Invalid Codex account metadata".into()));
        }
        serde_json::from_value(
            self.library_write(
                owner,
                "codex_account",
                id,
                &v.idempotency_key,
                v.expected_revision,
                serde_json::to_value(&v.value).map_err(storage_error)?,
            )
            .await?,
        )
        .map_err(storage_error)
    }
    pub async fn delete_codex_account(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        v: &DeleteLibraryEntry,
    ) -> Result<LibraryDeleted> {
        self.library_delete(owner, "codex_account", id, v).await
    }
    /// Returns an original response or a fresh one-shot admission. A prior unknown
    /// call is blocked, even if its acknowledgement was lost during restart.
    pub async fn prepare_codex_account_call(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        key: &str,
        revision: i64,
        operation: &str,
    ) -> Result<Option<Value>> {
        valid_text(key, 200)?;
        if key.chars().any(char::is_control)
            || revision <= 0
            || !matches!(operation, "login_browser" | "login_device" | "logout")
        {
            return Err(Error::Validation("Invalid Codex account action".into()));
        }
        let scope = owner.namespace();
        let digest = canonical_hash(&json!((id, revision, operation)));
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        sqlx::query("UPDATE codex_account_call_guard SET id=id WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let previous=sqlx::query("SELECT request_sha256,response FROM codex_account_calls WHERE scope=$1 AND operation_key=$2").bind(&scope).bind(key).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if let Some(row) = previous {
            if row
                .try_get::<String, _>("request_sha256")
                .map_err(storage_error)?
                != digest
            {
                return Err(Error::Conflict);
            }
            let response: Option<String> = row.try_get("response").map_err(storage_error)?;
            let result = response.ok_or(Error::UncertainEffect)?;
            let result = parse(&result)?;
            permit(self, &mut tx, owner, ContentAction::Write).await?;
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(result));
        }
        let saved:Option<i64>=sqlx::query_scalar("SELECT revision FROM library_config_entries WHERE scope=$1 AND kind='codex_account' AND id=$2 AND deleted=0")
            .bind(&scope).bind(id).fetch_optional(&mut *tx).await.map_err(storage_error)?;
        if saved.ok_or(Error::NotFound)? != revision {
            return Err(Error::Conflict);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM codex_account_calls WHERE scope=$1")
                .bind(&scope)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        if count >= 10000 {
            return Err(Error::BudgetExceeded);
        }
        sqlx::query("INSERT INTO codex_account_calls(scope,operation_key,account_id,request_sha256,response,created_at) VALUES($1,$2,$3,$4,NULL,$5)")
            .bind(&scope).bind(key).bind(id).bind(digest).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(None)
    }
    pub async fn complete_codex_account_call(
        &self,
        owner: LibraryOwner<'_>,
        id: &str,
        key: &str,
        response: &Value,
    ) -> Result<()> {
        if json_string(response)?.len() > 128 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        let changed=sqlx::query("UPDATE codex_account_calls SET response=$1 WHERE scope=$2 AND operation_key=$3 AND account_id=$4 AND response IS NULL")
            .bind(json_string(response)?).bind(owner.namespace()).bind(key).bind(id).execute(&mut *tx).await.map_err(storage_error)?;
        if changed.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        permit(self, &mut tx, owner, ContentAction::Write).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
}
