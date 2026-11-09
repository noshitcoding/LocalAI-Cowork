//! Whole-package inspection and reservation. Every role shares one original
//! signed package and one aggregate budget; no partial prepared result exists.
use super::*;
#[cfg(target_os = "linux")]
mod current;
#[cfg(target_os = "linux")]
pub use current::{InstalledPackage, InstalledPackagePolicy, NewRuntimeAdmission};
#[cfg(target_os = "linux")]
mod publication;
#[cfg(target_os = "linux")]
pub use publication::{
    PublicationAttempt, PublicationHistory, PublicationPhase, PublicationPolicy,
    PublicationReceipt, read_publication_history,
};

pub struct InstallationLimits {
    entries: usize,
    expanded_bytes: u64,
    path_bytes: usize,
}
impl InstallationLimits {
    pub fn new(entries: usize, expanded_bytes: u64, path_bytes: usize) -> Result<Self> {
        if entries == 0
            || entries > MAX_ENTRIES
            || expanded_bytes == 0
            || expanded_bytes > MAX_PACKAGE
            || path_bytes == 0
            || path_bytes > MAX_PATH_BYTES
        {
            return Err(Error::Archive("installation_limits"));
        }
        Ok(Self {
            entries,
            expanded_bytes,
            path_bytes,
        })
    }
}

struct ComponentInventory {
    component: usize,
    members: BTreeMap<String, Member>,
    expanded_bytes: u64,
}

pub struct InspectedInstallation {
    staged: StagedPackage,
    components: Vec<ComponentInventory>,
    entries: usize,
    expanded_bytes: u64,
    path_bytes: usize,
    depth: usize,
}

#[derive(Serialize)]
pub struct InstallationRoleReceipt<'a> {
    role: ComponentRole,
    component_sha256: &'a str,
    entries: usize,
    expanded_bytes: u64,
}
#[derive(Serialize)]
pub struct InstallationReceipt<'a> {
    manifest: &'a PackageManifest,
    manifest_sha256: &'a str,
    roles: Vec<InstallationRoleReceipt<'a>>,
    entries: usize,
    expanded_bytes: u64,
    path_bytes: usize,
    maximum_depth: usize,
    all_roles_inspected: bool,
    all_roles_prepared: bool,
    installed: bool,
    qualified: bool,
    execution_authority: bool,
    native_custody_reconstructible: bool,
}

impl StagedPackage {
    pub fn inspect_installation(
        mut self,
        limits: InstallationLimits,
    ) -> Result<InspectedInstallation> {
        self.verify()?;
        if self.manifest().components.len() != ROLES.len() {
            return Err(Error::Manifest);
        }
        let mut components = Vec::with_capacity(ROLES.len());
        let (mut entries, mut expanded_bytes, mut path_bytes, mut depth) =
            (0usize, 0u64, 0usize, 0usize);
        for (component, expected_role) in ROLES.iter().enumerate() {
            if self.manifest().components[component].role != *expected_role {
                return Err(Error::Manifest);
            }
            let remaining = ArchiveLimits {
                entries: limits
                    .entries
                    .checked_sub(entries)
                    .ok_or(Error::Archive("installation_entry_limit"))?,
                expanded_bytes: limits
                    .expanded_bytes
                    .checked_sub(expanded_bytes)
                    .ok_or(Error::Archive("installation_expanded_limit"))?
                    .min(MAX_COMPONENT),
                path_bytes: limits
                    .path_bytes
                    .checked_sub(path_bytes)
                    .ok_or(Error::Archive("installation_path_limit"))?,
            };
            let pinned = &mut self.files[component + 2];
            pinned.file.seek(SeekFrom::Start(0))?;
            let (members, bytes) = inspect(&mut pinned.file, pinned.bytes, &remaining)?;
            entries = entries
                .checked_add(members.len())
                .ok_or(Error::Archive("installation_entry_limit"))?;
            expanded_bytes = expanded_bytes
                .checked_add(bytes)
                .ok_or(Error::Archive("installation_expanded_limit"))?;
            for (path, member) in &members {
                let link_bytes = match &member.entry {
                    ArchiveEntry::Symlink { target } => target.len(),
                    _ => 0,
                };
                path_bytes = path_bytes
                    .checked_add(path.len())
                    .and_then(|n| n.checked_add(link_bytes))
                    .ok_or(Error::Archive("installation_path_limit"))?;
                depth = depth.max(path.split('/').count());
            }
            if entries > limits.entries
                || expanded_bytes > limits.expanded_bytes
                || path_bytes > limits.path_bytes
            {
                return Err(Error::Archive("installation_budget"));
            }
            components.push(ComponentInventory {
                component,
                members,
                expanded_bytes: bytes,
            });
        }
        self.verify()?;
        Ok(InspectedInstallation {
            staged: self,
            components,
            entries,
            expanded_bytes,
            path_bytes,
            depth,
        })
    }
}

impl InspectedInstallation {
    pub fn receipt(&mut self) -> Result<InstallationReceipt<'_>> {
        self.staged.verify()?;
        Ok(receipt(
            &self.staged,
            &self.components,
            self.entries,
            self.expanded_bytes,
            self.path_bytes,
            self.depth,
            false,
        ))
    }

    #[cfg(target_os = "linux")]
    pub fn prepare(
        mut self,
        area: StagingArea,
        policy: TargetDescriptorPolicy,
    ) -> Result<PreparedInstallation> {
        use super::extraction::{DescriptorReservation, ReservedTree};
        if unsafe { libc::geteuid() } != 0 {
            return Err(Error::Archive("root_manager_required"));
        }
        if self.expanded_bytes > area.payload_limit {
            return Err(Error::Archive("target_payload_limit"));
        }
        self.staged.verify()?;
        area.check()?;
        exact_entries(&area.directory, &[])?;
        // Two retained area descriptors for each fixed role root. This is
        // additional to target inodes, recursive ReadDir duplicates and helpers.
        let extra = ROLES
            .len()
            .checked_mul(2)
            .ok_or(Error::Archive("target_descriptor_budget"))?;
        let mut slots = DescriptorReservation::reserve_with_extra(
            &area.directory,
            self.entries,
            self.depth,
            extra,
            policy,
        )?;
        let mut trees = Vec::with_capacity(ROLES.len());
        for inventory in self.components {
            let name = role_directory(inventory.component);
            let role_area = StagingArea::create(
                area.directory.try_clone()?,
                &name,
                inventory.expanded_bytes.max(1),
            )?;
            let tree = ReservedTree::reserve(
                role_area,
                inventory.members,
                inventory.expanded_bytes,
                &mut slots,
            )?;
            trees.push((inventory.component, tree));
        }
        // Every role's directories, original handles and complete file storage
        // now exist. No payload byte has been copied into any role target yet.
        self.staged.verify()?;
        for (component, tree) in &mut trees {
            tree.copy(&mut self.staged, *component)?;
        }
        // All role file copies precede all link creation and directory sealing.
        for (_, tree) in &mut trees {
            tree.finish(&mut slots)?;
        }
        if !slots.is_empty() {
            return Err(Error::Staging);
        }
        native::sync_directory(&area.directory)?;
        let mut prepared = PreparedInstallation {
            staged: self.staged,
            area,
            trees,
            entries: self.entries,
            expanded_bytes: self.expanded_bytes,
            path_bytes: self.path_bytes,
            depth: self.depth,
        };
        prepared.verify()?;
        Ok(prepared)
    }
}

#[cfg(target_os = "linux")]
fn role_directory(component: usize) -> String {
    format!("component-{component}")
}

fn receipt<'a>(
    staged: &'a StagedPackage,
    inventories: &[ComponentInventory],
    entries: usize,
    expanded_bytes: u64,
    path_bytes: usize,
    depth: usize,
    prepared: bool,
) -> InstallationReceipt<'a> {
    InstallationReceipt {
        manifest: staged.manifest(),
        manifest_sha256: staged.manifest_sha256(),
        roles: inventories
            .iter()
            .map(|inventory| {
                let component = &staged.manifest().components[inventory.component];
                InstallationRoleReceipt {
                    role: component.role,
                    component_sha256: &component.sha256,
                    entries: inventory.members.len(),
                    expanded_bytes: inventory.expanded_bytes,
                }
            })
            .collect(),
        entries,
        expanded_bytes,
        path_bytes,
        maximum_depth: depth,
        all_roles_inspected: true,
        all_roles_prepared: prepared,
        installed: false,
        qualified: false,
        execution_authority: false,
        native_custody_reconstructible: false,
    }
}

#[cfg(target_os = "linux")]
pub struct PreparedInstallation {
    staged: StagedPackage,
    area: StagingArea,
    trees: Vec<(usize, super::extraction::ReservedTree)>,
    entries: usize,
    expanded_bytes: u64,
    path_bytes: usize,
    depth: usize,
}
#[cfg(target_os = "linux")]
impl PreparedInstallation {
    pub fn verify(&mut self) -> Result<()> {
        self.staged.verify()?;
        self.area.check()?;
        if self.trees.len() != ROLES.len() {
            return Err(Error::Manifest);
        }
        let names: Vec<String> = (0..ROLES.len()).map(role_directory).collect();
        let expected: Vec<&str> = names.iter().map(String::as_str).collect();
        exact_entries(&self.area.directory, &expected)?;
        let mut entries = 0usize;
        let mut bytes = 0u64;
        for ((component, tree), expected_component) in self.trees.iter().zip(0..ROLES.len()) {
            if *component != expected_component
                || self.staged.manifest().components[*component].role != ROLES[*component]
            {
                return Err(Error::Manifest);
            }
            tree.verify()?;
            entries = entries.checked_add(tree.entries()).ok_or(Error::Staging)?;
            bytes = bytes
                .checked_add(tree.expanded_bytes())
                .ok_or(Error::Staging)?;
        }
        if entries != self.entries || bytes != self.expanded_bytes {
            return Err(Error::Staging);
        }
        self.area.check()
    }
    pub fn receipt(&mut self) -> Result<InstallationReceipt<'_>> {
        self.verify()?;
        Ok(InstallationReceipt {
            manifest: self.staged.manifest(),
            manifest_sha256: self.staged.manifest_sha256(),
            roles: self
                .trees
                .iter()
                .map(|(component, tree)| {
                    let item = &self.staged.manifest().components[*component];
                    InstallationRoleReceipt {
                        role: item.role,
                        component_sha256: &item.sha256,
                        entries: tree.entries(),
                        expanded_bytes: tree.expanded_bytes(),
                    }
                })
                .collect(),
            entries: self.entries,
            expanded_bytes: self.expanded_bytes,
            path_bytes: self.path_bytes,
            maximum_depth: self.depth,
            all_roles_inspected: true,
            all_roles_prepared: true,
            installed: false,
            qualified: false,
            execution_authority: false,
            native_custody_reconstructible: false,
        })
    }
}
