//! Fresh authorization of one selected imported profile. The caller owns the TX.
use super::*;

impl TeamService {
    pub async fn configure_migration_provider_in_transaction(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: &Principal,
        team: Uuid,
        input: ConfigureTeamProvider,
    ) -> Result<TeamProvider> {
        let base_url =
            validate_provider(&input.base_url, &input.model, &self.allowed_http_origins)?;
        let secret = Zeroizing::new(input.api_key);
        if secret.is_empty() || secret.len() > 8192 || secret.chars().any(char::is_control) {
            return Err(Error::Validation(
                "Invalid freshly authorized provider key".into(),
            ));
        }
        self.manager(tx, actor, team, false).await?;
        let old = sqlx::query("SELECT revision FROM team_provider WHERE team_id=$1")
            .bind(team.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?;
        let revision = old
            .as_ref()
            .map(|r| r.try_get::<i64, _>("revision").map_err(storage_error))
            .transpose()?
            .unwrap_or(0);
        if revision != input.expected_revision {
            return Err(Error::Conflict);
        }
        let result = TeamProvider {
            team_id: team,
            base_url,
            model: input.model,
            revision: revision.checked_add(1).ok_or(Error::Conflict)?,
            key_configured: true,
        };
        let sealed = seal_bearer(self.keys.as_ref(), self.key_id, &scope(&result), &secret)
            .map_err(|_| Error::NativeKeyUnavailable)?;
        sqlx::query("INSERT INTO team_provider(team_id,revision,base_url,model,sealed_key) VALUES($1,$2,$3,$4,$5) ON CONFLICT(team_id) DO UPDATE SET revision=excluded.revision,base_url=excluded.base_url,model=excluded.model,sealed_key=excluded.sealed_key")
            .bind(team.to_string()).bind(result.revision).bind(&result.base_url).bind(&result.model)
            .bind(json_string(&sealed)?).execute(&mut **tx).await.map_err(storage_error)?;
        self.manager(tx, actor, team, false).await?;
        Ok(result)
    }
}
