use super::*;
use crate::codex_accounts::AccountOperation;
impl Runtime {
    pub(crate) async fn probe_configured_provider(
        &self,
        profile: &ProviderProfile,
        credential: Option<String>,
    ) -> Result<()> {
        if !matches!(profile.kind, ProviderKind::Codex) {
            return provider::probe(profile, credential, self.allow_test).await;
        }
        if credential.is_some() {
            return Err(Error::Forbidden);
        }
        let locator = self
            .store
            .codex_model_profile_account("installation-owner", profile)
            .await?;
        let broker = self
            .codex_broker()
            .ok_or_else(|| Error::Provider("codex_runtime_not_configured".into()))?;
        let status = broker
            .call(
                "installation-owner".into(),
                locator.account_id,
                AccountOperation::Read,
            )
            .await?;
        if status["account"].is_null() {
            return Err(Error::Provider("codex_account_sign_in_required".into()));
        }
        Ok(())
    }
    pub(crate) async fn codex_profile_model_catalog(
        &self,
        profile: &ProviderProfile,
    ) -> Result<Vec<String>> {
        let locator = self
            .store
            .codex_model_profile_account("installation-owner", profile)
            .await?;
        let broker = self
            .codex_broker()
            .ok_or_else(|| Error::Provider("codex_runtime_not_configured".into()))?;
        Ok(crate::codex_accounts::models(
            &broker
                .call(
                    "installation-owner".into(),
                    locator.account_id,
                    AccountOperation::Models,
                )
                .await?,
        )?
        .into_iter()
        .map(|model| model.id)
        .collect())
    }
}
