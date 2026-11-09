//! Bounded original journal ownership is mandatory for publication. Reopening
//! exposes history only; serialized local identifiers never reconstruct a permit.
use super::*;
use cap_std::fs::MetadataExt;
use serde::Deserialize;

const SLOT_BYTES: usize = 256 * 1024;
const FRAME_HEADER: usize = 36;
const FILES: [&str; 3] = ["owner.lock", "state.a", "state.b"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LocalId {
    device: u64,
    inode: u64,
}
impl LocalId {
    fn directory(dir: &Dir) -> Result<Self> {
        let m = dir.dir_metadata()?;
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
        })
    }
    fn file(file: &File) -> Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetRecord {
    parent: LocalId,
    name: String,
}
impl TargetRecord {
    pub(super) fn of(destination: &Destination) -> Result<Self> {
        Ok(Self {
            parent: LocalId::directory(&destination.parent)?,
            name: destination.name.clone(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Binding {
    journal: LocalId,
    owner: LocalId,
    slots: [LocalId; 2],
    source_parent: LocalId,
    source_root: LocalId,
    source_name: String,
    manifest: String,
    signature: String,
    manifest_sha256: String,
    release_sequence: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Intent,
    Published,
}
impl Phase {
    fn sequence(self) -> u64 {
        match self {
            Self::Prepared => 1,
            Self::Intent => 2,
            Self::Published => 3,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Record {
    format_version: u32,
    sequence: u64,
    phase: Phase,
    binding: Binding,
    target: Option<TargetRecord>,
}

pub(super) struct OriginalJournal {
    area: StagingArea,
    owner: File,
    slots: [File; 2],
    binding: Binding,
    current: Option<Record>,
    pending: Option<Record>,
    dispatch_disabled: bool,
}

fn locked(file: &File, mode: libc::c_int) -> Result<()> {
    // SAFETY: live original descriptor. Never unlock a foreign/opened owner.
    if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } != 0 {
        return Err(Error::Archive("publication_journal_owner_busy"));
    }
    Ok(())
}

fn metadata_bytes(staged: &mut StagedPackage, index: usize, limit: u64) -> Result<String> {
    let file = &mut staged.files[index].file;
    file.seek(SeekFrom::Start(0))?;
    let mut data = Vec::new();
    Read::by_ref(file).take(limit + 1).read_to_end(&mut data)?;
    file.seek(SeekFrom::Start(0))?;
    if data.is_empty() || data.len() as u64 > limit {
        return Err(Error::Staging);
    }
    String::from_utf8(data).map_err(|_| Error::Staging)
}

impl OriginalJournal {
    pub(super) fn initialize(
        root: &StagingArea,
        staged: &mut StagedPackage,
        trust: &TrustRoot,
        platform: Platform,
    ) -> Result<Self> {
        staged.verify()?;
        let name = format!("{}-journal", root.name);
        if !token(&name) {
            return Err(Error::Archive("publication_journal_name"));
        }
        let manifest = metadata_bytes(staged, 0, MAX_MANIFEST)?;
        let signature = metadata_bytes(staged, 1, 2048)?;
        let verified = verify_manifest(manifest.as_bytes(), signature.as_bytes(), trust, platform)?;
        if verified.manifest_sha256() != staged.manifest_sha256() {
            return Err(Error::Staging);
        }
        let area = StagingArea::create(root.parent.try_clone()?, &name, (SLOT_BYTES * 2) as u64)?;
        let owner = native::create_file(&area.directory, FILES[0])?;
        owner.sync_all()?;
        locked(&owner, libc::LOCK_EX)?;
        let slots = [
            native::create_file(&area.directory, FILES[1])?,
            native::create_file(&area.directory, FILES[2])?,
        ];
        for file in &slots {
            native::allocate(file, SLOT_BYTES as u64)?;
        }
        let binding = Binding {
            journal: LocalId::directory(&area.directory)?,
            owner: LocalId::file(&owner)?,
            slots: [LocalId::file(&slots[0])?, LocalId::file(&slots[1])?],
            source_parent: LocalId::directory(&root.parent)?,
            source_root: LocalId::directory(&root.directory)?,
            source_name: root.name.clone(),
            manifest,
            signature,
            manifest_sha256: staged.manifest_sha256().into(),
            release_sequence: staged.manifest().release_sequence,
        };
        let mut journal = Self {
            area,
            owner,
            slots,
            binding,
            current: None,
            pending: None,
            dispatch_disabled: false,
        };
        journal.persist(Phase::Prepared, None)?;
        native::sync_directory(&journal.area.directory)?;
        native::sync_directory(&journal.area.parent)?;
        staged.verify()?;
        journal.verify_original()?;
        Ok(journal)
    }

    fn verify_original(&self) -> Result<()> {
        self.area.check()?;
        exact_entries(&self.area.directory, &FILES)?;
        if LocalId::directory(&self.area.directory)? != self.binding.journal
            || LocalId::file(&self.owner)? != self.binding.owner
            || LocalId::file(&native::pin_file(&self.area.directory, FILES[0])?)?
                != self.binding.owner
        {
            return Err(Error::Archive("publication_journal_original_changed"));
        }
        native::file_identity(&self.owner)?;
        native::check_allocation(&self.owner, 0)?;
        locked(&self.owner, libc::LOCK_EX)?;
        for (index, file) in self.slots.iter().enumerate() {
            native::check_allocation(file, SLOT_BYTES as u64)?;
            native::file_identity(file)?;
            if LocalId::file(file)? != self.binding.slots[index]
                || LocalId::file(&native::pin_file(&self.area.directory, FILES[index + 1])?)?
                    != self.binding.slots[index]
            {
                return Err(Error::Archive("publication_journal_original_changed"));
            }
        }
        self.area.check()
    }

    fn persist(&mut self, phase: Phase, target: Option<TargetRecord>) -> Result<()> {
        let desired = Record {
            format_version: 1,
            sequence: phase.sequence(),
            phase,
            binding: self.binding.clone(),
            target,
        };
        if let Some(pending) = &self.pending {
            if pending != &desired {
                return Err(Error::Archive("publication_journal_pending_changed"));
            }
        } else {
            self.pending = Some(desired.clone());
        }
        // Retain the exact pending frame and all originals before the first write.
        let result = (|| {
            self.verify_original()?;
            let payload = serde_json::to_vec(&desired)?;
            if payload.len() > SLOT_BYTES - FRAME_HEADER {
                return Err(Error::Archive("publication_journal_frame_limit"));
            }
            let mut frame = vec![0u8; SLOT_BYTES];
            frame[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            frame[4..36].copy_from_slice(&Sha256::digest(&payload));
            frame[36..36 + payload.len()].copy_from_slice(&payload);
            let index = (desired.sequence % 2) as usize;
            let file = &mut self.slots[index];
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&frame)?;
            file.sync_all()?;
            self.verify_original()?;
            if read_frame(&mut self.slots[index], index)?.as_ref() != Some(&desired) {
                return Err(Error::Archive("publication_journal_readback"));
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.current = Some(desired);
                self.pending = None;
                Ok(())
            }
            Err(error) => {
                self.dispatch_disabled = true;
                Err(error)
            }
        }
    }

    pub(super) fn intent(
        &mut self,
        root: &StagingArea,
        staged: &StagedPackage,
        target: &Destination,
    ) -> Result<()> {
        if self.dispatch_disabled
            || self
                .current
                .as_ref()
                .is_none_or(|r| r.phase != Phase::Prepared)
            || self.pending.is_some()
        {
            return Err(Error::Archive("publication_journal_dispatch_disabled"));
        }
        self.verify_binding(root, staged)?;
        self.persist(Phase::Intent, Some(TargetRecord::of(target)?))
    }

    pub(super) fn prepared(&self, root: &StagingArea, staged: &StagedPackage) -> Result<()> {
        self.verify_binding(root, staged)?;
        let record = self.current.as_ref().ok_or(Error::Staging)?;
        if self.dispatch_disabled || self.pending.is_some() || record.phase != Phase::Prepared {
            return Err(Error::Archive("publication_journal_unconfirmed"));
        }
        let mut file = &self.slots[1];
        if read_frame(&mut file, 1)?.as_ref() != Some(record) {
            return Err(Error::Archive("publication_journal_readback"));
        }
        self.verify_original()
    }

    fn verify_binding(&self, root: &StagingArea, staged: &StagedPackage) -> Result<()> {
        self.verify_original()?;
        if LocalId::directory(&root.directory)? != self.binding.source_root
            || LocalId::directory(&root.parent)? != self.binding.source_parent
            || root.name != self.binding.source_name
            || staged.manifest_sha256() != self.binding.manifest_sha256
            || staged.manifest().release_sequence != self.binding.release_sequence
        {
            return Err(Error::Archive("publication_journal_binding"));
        }
        Ok(())
    }

    pub(super) fn authorize(
        &self,
        root: &StagingArea,
        staged: &StagedPackage,
        target: &Destination,
    ) -> Result<()> {
        self.verify_binding(root, staged)?;
        let expected = self.current.as_ref().ok_or(Error::Staging)?;
        if self.dispatch_disabled
            || self.pending.is_some()
            || expected.phase != Phase::Intent
            || expected.target.as_ref() != Some(&TargetRecord::of(target)?)
        {
            return Err(Error::Archive("publication_journal_dispatch_disabled"));
        }
        let mut file = &self.slots[(expected.sequence % 2) as usize];
        if read_frame(&mut file, (expected.sequence % 2) as usize)?.as_ref() != Some(expected) {
            return Err(Error::Archive("publication_journal_readback"));
        }
        self.verify_original()
    }

    pub(super) fn confirm(
        &mut self,
        root: &StagingArea,
        staged: &StagedPackage,
        target: &Destination,
    ) -> Result<()> {
        self.verify_binding(root, staged)?;
        let target = TargetRecord::of(target)?;
        if self
            .current
            .as_ref()
            .is_some_and(|r| r.phase == Phase::Published && r.target.as_ref() == Some(&target))
        {
            return self.verify_published(root, staged, &target);
        }
        if self
            .current
            .as_ref()
            .is_none_or(|r| r.phase != Phase::Intent || r.target.as_ref() != Some(&target))
        {
            return Err(Error::Archive("publication_journal_completion_invalid"));
        }
        // A failed dispatch is never enabled again. Only this original held
        // completion may retry its exact frame; no rename is performed here.
        self.persist(Phase::Published, Some(target))
    }

    fn verify_published(
        &self,
        root: &StagingArea,
        staged: &StagedPackage,
        target: &TargetRecord,
    ) -> Result<()> {
        self.verify_binding(root, staged)?;
        let record = self.current.as_ref().ok_or(Error::Staging)?;
        if record.phase != Phase::Published
            || record.target.as_ref() != Some(target)
            || self.pending.is_some()
        {
            return Err(Error::Archive("publication_journal_unconfirmed"));
        }
        let mut file = &self.slots[1];
        if read_frame(&mut file, 1)?.as_ref() != Some(record) {
            return Err(Error::Archive("publication_journal_readback"));
        }
        self.verify_original()
    }

    pub(super) fn completed(
        &self,
        root: &StagingArea,
        staged: &StagedPackage,
        target: &Destination,
    ) -> Result<()> {
        self.verify_published(root, staged, &TargetRecord::of(target)?)
    }
    pub(super) fn name(&self) -> &str {
        &self.area.name
    }
}

fn read_frame(file: &mut impl ReadSeek, index: usize) -> Result<Option<Record>> {
    file.seek(SeekFrom::Start(0))?;
    let mut frame = vec![0u8; SLOT_BYTES];
    file.read_exact(&mut frame)?;
    if frame.iter().all(|b| *b == 0) {
        return Ok(None);
    }
    let bytes = u32::from_le_bytes(frame[..4].try_into().map_err(|_| Error::Staging)?) as usize;
    if bytes == 0
        || bytes > SLOT_BYTES - FRAME_HEADER
        || Sha256::digest(&frame[36..36 + bytes]).as_slice() != &frame[4..36]
        || frame[36 + bytes..].iter().any(|b| *b != 0)
    {
        return Err(Error::Archive("publication_journal_frame"));
    }
    let record: Record = serde_json::from_slice(&frame[36..36 + bytes])?;
    if record.format_version != 1
        || record.sequence != record.phase.sequence()
        || record.sequence as usize % 2 != index
        || (record.phase == Phase::Prepared) != record.target.is_none()
    {
        return Err(Error::Archive("publication_journal_phase"));
    }
    if !token(&record.binding.source_name)
        || !valid_hash(&record.binding.manifest_sha256)
        || record.target.as_ref().is_some_and(|target| {
            target.name != format!("version-{}", record.binding.manifest_sha256)
        })
    {
        return Err(Error::Archive("publication_journal_binding"));
    }
    Ok(Some(record))
}
trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// Informational only. It owns no original publication, dispatch or cleanup
/// capability and provides no method that can activate, rename or release data.
#[derive(Serialize)]
pub struct PublicationHistory {
    manifest: PackageManifest,
    manifest_sha256: String,
    last_valid_phase: Phase,
    last_valid_sequence: u64,
    version_directory: Option<String>,
    frames_degraded: bool,
    physical_publication_unconfirmed_after_process_loss: bool,
    status_only: bool,
    can_dispatch: bool,
    can_activate: bool,
    can_reclaim: bool,
    native_custody_reconstructible: bool,
    execution_authority: bool,
}
pub fn read_publication_history(
    directory: Dir,
    trust: &TrustRoot,
    platform: Platform,
) -> Result<PublicationHistory> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(Error::Archive("root_manager_required"));
    }
    native::private_directory(&directory)?;
    exact_entries(&directory, &FILES)?;
    let owner = native::pin_file(&directory, FILES[0])?;
    native::check_allocation(&owner, 0)?;
    locked(&owner, libc::LOCK_SH)?;
    let mut records = Vec::new();
    let mut degraded = false;
    let mut slots = [
        native::pin_file(&directory, FILES[1])?,
        native::pin_file(&directory, FILES[2])?,
    ];
    for (index, file) in slots.iter_mut().enumerate() {
        native::check_allocation(file, SLOT_BYTES as u64)?;
        match read_frame(file, index) {
            Ok(Some(record)) => records.push(record),
            Ok(None) => {}
            Err(_) => degraded = true,
        }
    }
    records.sort_by_key(|record| record.sequence);
    let latest = records
        .last()
        .ok_or(Error::Archive("publication_journal_no_valid_frame"))?;
    let binding = &latest.binding;
    if records.iter().any(|r| r.binding != *binding)
        || LocalId::directory(&directory)? != binding.journal
        || LocalId::file(&owner)? != binding.owner
        || LocalId::file(&slots[0])? != binding.slots[0]
        || LocalId::file(&slots[1])? != binding.slots[1]
    {
        return Err(Error::Archive("publication_journal_original_changed"));
    }
    let verified = verify_manifest(
        binding.manifest.as_bytes(),
        binding.signature.as_bytes(),
        trust,
        platform,
    )?;
    if verified.manifest_sha256() != binding.manifest_sha256
        || verified.manifest().release_sequence != binding.release_sequence
    {
        return Err(Error::Archive("publication_journal_binding"));
    }
    Ok(PublicationHistory {
        manifest: verified.manifest().clone(),
        manifest_sha256: binding.manifest_sha256.clone(),
        last_valid_phase: latest.phase,
        last_valid_sequence: latest.sequence,
        version_directory: latest.target.as_ref().map(|t| t.name.clone()),
        frames_degraded: degraded,
        physical_publication_unconfirmed_after_process_loss: true,
        status_only: true,
        can_dispatch: false,
        can_activate: false,
        can_reclaim: false,
        native_custody_reconstructible: false,
        execution_authority: false,
    })
}
