//! Independent physical deadline fence. A frozen ancestor is never passed as
//! the runtime's writable leaf. Late task creation cannot thaw that ancestor.
//! Shim control remains separate so acknowledged delete can still complete.
use super::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct BoundaryObservation {
    pub triggered: bool,
    pub frozen: bool,
    pub kill_requested: bool,
    pub failed: bool,
}
struct State {
    stop: AtomicBool,
    trigger: AtomicBool,
    frozen: AtomicBool,
    killed: AtomicBool,
    failed: AtomicBool,
}
pub struct TaskBoundary {
    group: TaskCgroup,
    workload: Dir,
    workload_id: NativeId,
    sandbox: Dir,
    sandbox_id: NativeId,
    shim: Dir,
    shim_id: NativeId,
    state: Arc<State>,
    worker: Option<std::thread::JoinHandle<()>>,
    process: Option<crate::containerd::deadline_process::DeadlineProcess>,
    deadline: Instant,
}
impl TaskBoundary {
    /// Consumes the unpublished cgroup. Error/drop never removes a published
    /// boundary. Cancellation uses only this exact native owned subtree.
    pub fn arm(
        group: TaskCgroup,
        deadline: Instant,
        cancel: Arc<AtomicBool>,
        executable: &crate::containerd::deadline_process::FenceExecutable,
    ) -> Result<Self> {
        group.verify()?;
        if !empty(&group.directory)? || !directories(&group.directory)?.is_empty() {
            return Err(Error::Cgroup);
        }
        write(
            &group.directory,
            "cgroup.subtree_control",
            "+cpu +memory +pids",
        )?;
        group.directory.create_dir("workload")?;
        let workload = group.directory.open_dir_nofollow("workload")?;
        let workload_id = NativeId::capture(&workload)?;
        write(&workload, "cgroup.subtree_control", "+cpu +memory +pids")?;
        workload.create_dir("sandbox")?;
        let sandbox = workload.open_dir_nofollow("sandbox")?;
        let sandbox_id = NativeId::capture(&sandbox)?;
        group.directory.create_dir("shim")?;
        let shim = group.directory.open_dir_nofollow("shim")?;
        let shim_id = NativeId::capture(&shim)?;
        let process = crate::containerd::deadline_process::DeadlineProcess::start(
            executable, &workload, deadline,
        )?;
        let state = Arc::new(State {
            stop: AtomicBool::new(false),
            trigger: AtomicBool::new(false),
            frozen: AtomicBool::new(false),
            killed: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        });
        let watcher = state.clone();
        let owned_workload = workload.try_clone()?;
        let group_dir = group.directory.try_clone()?;
        let group_id = group.id;
        let worker = std::thread::Builder::new()
            .name("cowork-task-deadline".into())
            .spawn(move || {
                loop {
                    if watcher.stop.load(Ordering::Acquire) {
                        return;
                    }
                    if cancel.load(Ordering::Acquire)
                        || watcher.trigger.load(Ordering::Acquire)
                        || Instant::now() >= deadline
                    {
                        watcher.trigger.store(true, Ordering::Release);
                        let result = (|| -> Result<()> {
                            group_id.require(&group_dir)?;
                            workload_id.require(&owned_workload)?;
                            workload_id.require(&group_dir.open_dir_nofollow("workload")?)?;
                            write(&owned_workload, "cgroup.freeze", "1")?;
                            // kill is complementary to freeze, never a completion or
                            // capacity-release receipt. The freeze stays persistent.
                            write(&owned_workload, "cgroup.kill", "1")?;
                            watcher.killed.store(true, Ordering::Release);
                            let end = Instant::now() + Duration::from_secs(2);
                            while Instant::now() < end {
                                workload_id.require(&owned_workload)?;
                                if read(&owned_workload, "cgroup.events")?
                                    .lines()
                                    .any(|line| line == "frozen 1")
                                {
                                    watcher.frozen.store(true, Ordering::Release);
                                    return Ok(());
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            Err(Error::Cgroup)
                        })();
                        if result.is_err() {
                            watcher.failed.store(true, Ordering::Release);
                        }
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })?;
        Ok(Self {
            group,
            workload,
            workload_id,
            sandbox,
            sandbox_id,
            shim,
            shim_id,
            state,
            worker: Some(worker),
            process: Some(process),
            deadline,
        })
    }
    pub fn sandbox_path(&self) -> String {
        format!("{}/workload/sandbox", self.group.oci_path)
    }
    pub fn shim_path(&self) -> String {
        format!("{}/shim", self.group.oci_path)
    }
    pub(in crate::containerd) fn shim_placement(&self) -> Result<cowork_executor::CgroupPlacement> {
        self.verify()?;
        cowork_executor::CgroupPlacement::from_directory(self.shim.open(".")?.into_std())
            .map_err(|_| Error::Cgroup)
    }
    pub fn limits(&self) -> HardLimits {
        self.group.limits
    }
    pub fn request_stop(&self) {
        self.state.trigger.store(true, Ordering::Release);
    }
    pub fn observation(&self) -> BoundaryObservation {
        BoundaryObservation {
            triggered: self.state.trigger.load(Ordering::Acquire),
            frozen: self.state.frozen.load(Ordering::Acquire),
            kill_requested: self.state.killed.load(Ordering::Acquire),
            failed: self.state.failed.load(Ordering::Acquire),
        }
    }
    pub fn verify(&self) -> Result<()> {
        self.group.verify()?;
        self.workload_id.require(&self.workload)?;
        self.sandbox_id.require(&self.sandbox)?;
        self.shim_id.require(&self.shim)?;
        self.workload_id
            .require(&self.group.directory.open_dir_nofollow("workload")?)?;
        self.sandbox_id
            .require(&self.workload.open_dir_nofollow("sandbox")?)?;
        self.shim_id
            .require(&self.group.directory.open_dir_nofollow("shim")?)?;
        Ok(())
    }
    pub(in crate::containerd) fn dispatch_ready(&self) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err(Error::TaskBoundary {
                reason: "deadline_elapsed",
            });
        }
        if self.observation().triggered {
            return Err(Error::TaskBoundary {
                reason: "stop_requested",
            });
        }
        self.verify().map_err(|_| Error::TaskBoundary {
            reason: "native_scope_changed",
        })?;
        self.process
            .as_ref()
            .ok_or(Error::Cgroup)?
            .verify_alive()
            .map_err(|_| Error::TaskBoundary {
                reason: "independent_fence_not_live",
            })
    }
    pub fn workload_empty(&self) -> Result<bool> {
        self.group.verify()?;
        self.workload_id.require(&self.workload)?;
        self.workload_id
            .require(&self.group.directory.open_dir_nofollow("workload")?)?;
        empty(&self.workload)
    }
    /// Only the live adapter after all creator RPCs are acknowledged and task
    /// deletion is acknowledged may retire. No public/replayable boolean proof.
    pub(in crate::containerd) fn retire_deleted(mut self) -> Result<()> {
        self.group.verify()?;
        self.workload_id.require(&self.workload)?;
        self.workload_id
            .require(&self.group.directory.open_dir_nofollow("workload")?)?;
        self.shim_id.require(&self.shim)?;
        self.shim_id
            .require(&self.group.directory.open_dir_nofollow("shim")?)?;
        let sandbox_present = match self.workload.open_dir_nofollow("sandbox") {
            Ok(current) => {
                self.sandbox_id.require(&current)?;
                self.sandbox_id.require(&self.sandbox)?;
                if !directories(&current)?.is_empty() {
                    return Err(Error::Cgroup);
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // runsc deletes its exact OCI leaf during acknowledged delete.
                // A missing path is accepted only with the retained unlinked
                // original native handle; a replacement is always rejected.
                if self.sandbox.open(".")?.into_std().metadata()?.nlink() != 0 {
                    return Err(Error::Cgroup);
                }
                false
            }
            Err(error) => return Err(error.into()),
        };
        if !empty(&self.workload)? {
            return Err(Error::Cgroup);
        }
        self.state.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| Error::Cgroup)?;
        }
        // Shim shutdown can finish shortly after TaskDelete's acknowledgement.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !empty(&self.shim)? && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !empty(&self.shim)? || !directories(&self.shim)?.is_empty() {
            return Err(Error::Cgroup);
        }
        // All creators/deletes were acknowledged and both native branches are
        // empty. Disarm before unlink so a concurrent deadline cannot write to
        // an already-unlinked cgroup and turn a known stop into a false error.
        self.process.take().ok_or(Error::Cgroup)?.disarm()?;
        if sandbox_present {
            self.workload.remove_dir("sandbox")?;
        }
        self.group.directory.remove_dir("workload")?;
        self.group.directory.remove_dir("shim")?;
        // Removing the exactly owned empty parent is a resource operation only;
        // caller's creator/delete proof is the separate prerequisite.
        self.group.verify()?;
        if !empty(&self.group.directory)? || !directories(&self.group.directory)?.is_empty() {
            return Err(Error::Cgroup);
        }
        self.group.parent.remove_dir(&self.group.name)?;
        Ok(())
    }
}
impl Drop for TaskBoundary {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.state.trigger.store(true, Ordering::Release);
            let _ = worker.join();
        }
        // Uncertain boundaries stay frozen/booked, including after future drop.
        // There is deliberately no lazy removal or automatic thaw here.
    }
}
