//! Platform-independent scheduling; original workers own external IO and cleanup.
use super::*;
impl AndroidDeviceServices {
    pub async fn run_private_schedules(
        self: Arc<Self>,
        stop: CancellationToken,
        admission: Arc<tokio::sync::RwLock<()>>,
    ) {
        let mut clock = tokio::time::interval(Duration::from_secs(5));
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {_=stop.cancelled()=>break,_=clock.tick()=>{}}
            let guard = tokio::select! {_=stop.cancelled()=>break,guard=admission.read()=>guard};
            if stop.is_cancelled() {
                break;
            }
            let runner = self.private_runner_binding();
            let tick = self.store.android_schedule_tick(
                self.private_server_available(),
                runner.as_ref(),
                || !stop.is_cancelled(),
            );
            let result = tokio::select! {_=stop.cancelled()=>break,result=tick=>result};
            drop(guard);
            if result.is_err() {
                tracing::warn!("Private schedule admission remains durable; tick did not commit");
            }
        }
    }
}
