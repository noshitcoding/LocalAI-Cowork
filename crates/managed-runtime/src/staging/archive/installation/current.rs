//! Fresh installed-byte custody. This is not old publication/task custody and
//! never grants activation, workload execution, cleanup or qualification alone.
use super::*;
use crate::staging::release_floor::{FloorAnchor, ReleaseFloor};
use std::sync::Mutex;

/// No Clone/Deserialize and no constructor from metadata, a receipt or stored
/// local IDs. Every source and expanded inode was opened and verified here.
pub struct InstalledPackage {
    root: StagingArea,
    content: Mutex<PreparedInstallation>,
    floor: FloorAnchor,
    manifest_sha256: String,
    manifest: PackageManifest,
}

/// Opaque current restrictive-policy lock; callers keep it through new dispatch.
/// It exposes no advance, execution, cleanup or recovered-operation methods.
pub struct NewRuntimeAdmission<'a> {
    _floor: ReleaseFloor,
    _package: &'a InstalledPackage,
}

pub struct InstalledPackagePolicy {
    total_payload_limit: u64,
    limits: InstallationLimits,
    descriptors: TargetDescriptorPolicy,
}
impl InstalledPackagePolicy {
    pub fn new(
        total_payload_limit: u64,
        limits: InstallationLimits,
        descriptors: TargetDescriptorPolicy,
    ) -> Result<Self> {
        if total_payload_limit == 0 || total_payload_limit > MAX_PACKAGE {
            return Err(Error::Archive("installed_payload_policy"));
        }
        Ok(Self {
            total_payload_limit,
            limits,
            descriptors,
        })
    }
}

impl InstalledPackage {
    pub fn open(
        versions: Dir,
        manifest_sha256: &str,
        trust: &TrustRoot,
        platform: Platform,
        floor: ReleaseFloor,
        policy: InstalledPackagePolicy,
    ) -> Result<Self> {
        let InstalledPackagePolicy {
            total_payload_limit,
            limits,
            descriptors,
        } = policy;
        if !valid_hash(manifest_sha256) {
            return Err(Error::Archive("installed_package_identity"));
        }
        let root = StagingArea::open_current(
            versions,
            &format!("version-{manifest_sha256}"),
            total_payload_limit,
        )?;
        exact_entries(&root.directory, &["package", "runtime"])?;
        let staged = StagedPackage::open_current_inputs(
            root.directory.try_clone()?,
            trust,
            platform,
            manifest_sha256,
            total_payload_limit,
        )?;
        floor.check_candidate(&staged.verified)?;
        let anchor = floor.anchor()?;
        let input_bytes = staged
            .manifest()
            .components
            .iter()
            .try_fold(0u64, |n, c| n.checked_add(c.bytes).ok_or(Error::Staging))?;
        let inspected = staged.inspect_installation(limits)?;
        if input_bytes
            .checked_add(inspected.expanded_bytes)
            .is_none_or(|n| n > total_payload_limit)
        {
            return Err(Error::Archive("installed_combined_payload_budget"));
        }
        let runtime = StagingArea::open_current(
            root.directory.try_clone()?,
            "runtime",
            inspected.expanded_bytes.max(1),
        )?;
        let prepared = inspected.open_current(runtime, descriptors)?;
        let manifest = prepared.staged.manifest().clone();
        let value = Self {
            root,
            content: Mutex::new(prepared),
            floor: anchor,
            manifest_sha256: manifest_sha256.into(),
            manifest,
        };
        value.verify()?;
        // This lock was held over the initial full proof. The original policy
        // anchor keeps identity handles only, allowing subsequent updates.
        drop(floor);
        Ok(value)
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
    pub fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }

    pub fn verify(&self) -> Result<()> {
        self.root.check()?;
        exact_entries(&self.root.directory, &["package", "runtime"])?;
        let mut content = self
            .content
            .lock()
            .map_err(|_| Error::Archive("installed_capability_poisoned"))?;
        if content.staged.manifest_sha256() != self.manifest_sha256 {
            return Err(Error::Archive("installed_package_identity"));
        }
        content.verify()?;
        self.root.check()
    }

    pub fn admit_new_start(&self) -> Result<NewRuntimeAdmission<'_>> {
        self.verify()?;
        let content = self
            .content
            .lock()
            .map_err(|_| Error::Archive("installed_capability_poisoned"))?;
        let floor = self.floor.admit(&content.staged.verified)?;
        drop(content);
        self.verify()?;
        Ok(NewRuntimeAdmission {
            _floor: floor,
            _package: self,
        })
    }
}

impl InspectedInstallation {
    fn open_current(
        self,
        area: StagingArea,
        policy: TargetDescriptorPolicy,
    ) -> Result<PreparedInstallation> {
        use super::super::extraction::{DescriptorReservation, ReservedTree};
        area.check()?;
        let names: Vec<_> = (0..ROLES.len()).map(role_directory).collect();
        exact_entries(
            &area.directory,
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        )?;
        let mut slots = DescriptorReservation::reserve_with_extra(
            &area.directory,
            self.entries,
            self.depth,
            ROLES.len() * 2,
            policy,
        )?;
        let mut trees = Vec::with_capacity(ROLES.len());
        for inventory in self.components {
            let role = StagingArea::open_current(
                area.directory.try_clone()?,
                &role_directory(inventory.component),
                inventory.expanded_bytes.max(1),
            )?;
            let tree = ReservedTree::open_current(
                role,
                inventory.members,
                inventory.expanded_bytes,
                &mut slots,
            )?;
            trees.push((inventory.component, tree));
        }
        if !slots.is_empty() {
            return Err(Error::Archive("installed_descriptor_budget"));
        }
        let mut value = PreparedInstallation {
            staged: self.staged,
            area,
            trees,
            entries: self.entries,
            expanded_bytes: self.expanded_bytes,
            path_bytes: self.path_bytes,
            depth: self.depth,
        };
        value.verify()?;
        Ok(value)
    }
}
