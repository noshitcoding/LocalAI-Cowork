//! Explicit user continuation through the configured original private runner.
use super::*;
use cowork_contracts::{AndroidResumeRun, AndroidRunResumeReceipt};
use cowork_identity::Principal;
use cowork_storage::android_run_recovery::AndroidPrivateResumePrepared;
use uuid::Uuid;

impl AndroidDeviceServices {
    pub fn private_recovery_observer(&self) -> &str {
        &self.recovery_observer
    }
    pub async fn resume_private_run(
        &self,
        actor: &Principal,
        run: Uuid,
        request: &AndroidResumeRun,
    ) -> Result<AndroidRunResumeReceipt> {
        match self
            .store
            .android_private_run_resume_prepare(
                actor,
                run,
                request,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?
        {
            AndroidPrivateResumePrepared::Original(receipt) => Ok(receipt),
            AndroidPrivateResumePrepared::Pending(snapshot) => {
                if snapshot.requires_cleanup() {
                    // Positive original no-start closure needs no guest. The
                    // independent final SQL proof still requires every original
                    // session and physical container to be safely settled.
                    self.store
                        .android_private_terminal_recover(self.keys.as_ref())
                        .await?;
                    self.store.android_private_browser_recover().await?;
                    self.store.android_private_github_recover().await?;
                    // Existing cleanup stores its original physical stop before
                    // removal; missing processes are never substituted for it.
                    if let Some(runner) = &self.private_runner {
                        tokio::time::timeout(Duration::from_secs(60), runner.recover())
                            .await
                            .map_err(|_| Error::UncertainEffect)??;
                    }
                }
                self.store
                    .android_private_run_resume_finish(
                        actor,
                        request,
                        &snapshot,
                        self.keys.as_ref(),
                        self.key_id,
                    )
                    .await
            }
        }
    }
}
