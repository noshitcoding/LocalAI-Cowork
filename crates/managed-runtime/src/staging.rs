//! Bounded copying of a signed package into an already private management area.
//!
//! This is not extraction, activation, qualification or an execution grant. The
//! installer must provision a separately bounded filesystem before using this
//! API. Failed attempts remain bounded and visible; they are never recursively
//! deleted, adopted after restart, or substituted for a previous installation.
pub mod archive;
#[cfg(target_os = "linux")]
mod current_inputs;
mod native;
#[cfg(target_os = "linux")]
pub mod release_floor;

use crate::*;
use cap_std::fs::File;
use std::io::{Seek, SeekFrom, Write};

/// One newly created attempt, consumed by its only staging operation. Neither
/// this nor StagedPackage can be cloned or reconstructed from serialized state.
pub struct StagingArea {
    parent: Dir,
    directory: Dir,
    name: String,
    parent_id: native::Identity,
    directory_id: native::Identity,
    payload_limit: u64,
}

struct PinnedFile {
    name: String,
    file: File,
    id: native::Identity,
    bytes: u64,
    sha256: String,
}

pub struct StagedPackage {
    area: StagingArea,
    verified: VerifiedPackage,
    files: Vec<PinnedFile>,
}

/// Informational readback of the original staged capability. It contains no
/// native IDs or authority and cannot reconstruct custody after process exit.
#[derive(Serialize)]
pub struct StagingReceipt<'a> {
    manifest: &'a PackageManifest,
    manifest_sha256: &'a str,
    signature_verified: bool,
    payload_verified: bool,
    staged: bool,
    installed: bool,
    qualified: bool,
    execution_authority: bool,
    native_custody_reconstructible: bool,
}

impl StagingArea {
    /// Parent is supplied by trusted installer configuration, never the package.
    /// It must already be owned by the management user and private. No parent
    /// permissions, global configuration or existing directory are changed.
    /// payload_limit bounds signed logical payload bytes; filesystem metadata
    /// and allocation rounding require the installer's separate volume quota.
    pub fn create(parent: Dir, name: &str, payload_limit: u64) -> Result<Self> {
        if !token(name) || payload_limit == 0 || payload_limit > MAX_PACKAGE {
            return Err(Error::Staging);
        }
        native::private_directory(&parent)?;
        let parent_id = native::directory_identity(&parent)?;
        #[cfg(unix)]
        let mut builder = cap_std::fs::DirBuilder::new();
        #[cfg(windows)]
        let builder = cap_std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        parent.create_dir_with(name, &builder)?;
        let directory = parent.open_dir_nofollow(name)?;
        native::private_directory(&directory)?;
        let directory_id = native::directory_identity(&directory)?;
        native::sync_directory(&parent)?;
        let area = Self {
            parent,
            directory,
            name: name.into(),
            parent_id,
            directory_id,
            payload_limit,
        };
        area.check()?;
        Ok(area)
    }

    fn check(&self) -> Result<()> {
        native::private_directory(&self.parent)?;
        native::private_directory(&self.directory)?;
        if native::directory_identity(&self.parent)? != self.parent_id
            || native::directory_identity(&self.directory)? != self.directory_id
            || native::directory_identity(&self.parent.open_dir_nofollow(&self.name)?)?
                != self.directory_id
        {
            return Err(Error::Staging);
        }
        Ok(())
    }

    pub fn stage(
        self,
        source: &Dir,
        trust: &TrustRoot,
        platform: Platform,
    ) -> Result<StagedPackage> {
        self.check()?;
        exact_entries(&self.directory, &[])?;
        exact_entries(source, &["manifest.json", "manifest.sig.json", "payload"])?;
        let source_id = native::directory_identity(source)?;
        let manifest = read_small(source, "manifest.json", MAX_MANIFEST)?;
        let signature = read_small(source, "manifest.sig.json", 2048)?;
        let verified = verify_manifest(&manifest, &signature, trust, platform)?;
        let total = verified
            .manifest
            .components
            .iter()
            .try_fold(0u64, |n, c| n.checked_add(c.bytes).ok_or(Error::Staging))?;
        if total > self.payload_limit {
            return Err(Error::Staging);
        }
        let payload = source.open_dir_nofollow("payload")?;
        let payload_id = native::directory_identity(&payload)?;
        let names: Vec<_> = verified
            .manifest
            .components
            .iter()
            .map(|c| c.filename.as_str())
            .collect();
        exact_entries(&payload, &names)?;

        // All storage must be physically allocated before copying any payload.
        // No sparse fallback on an unsupported or full filesystem.
        let mut outputs = Vec::with_capacity(11);
        for (name, bytes) in [
            ("manifest.json".to_owned(), manifest.len() as u64),
            ("manifest.sig.json".to_owned(), signature.len() as u64),
        ]
        .into_iter()
        .chain(
            verified
                .manifest
                .components
                .iter()
                .enumerate()
                .map(|(i, c)| (format!("component-{i}.bin"), c.bytes)),
        ) {
            self.check()?;
            let file = native::create_file(&self.directory, &name)?;
            let id = native::file_identity(&file)?;
            native::allocate(&file, bytes)?;
            outputs.push((name, file, id));
        }
        let mut pinned = Vec::with_capacity(11);
        for (index, (name, mut output, id)) in outputs.into_iter().enumerate() {
            self.check()?;
            let (bytes, digest) = if index < 2 {
                let bytes = if index == 0 { &manifest } else { &signature };
                output.write_all(bytes)?;
                (bytes.len() as u64, sha256(bytes))
            } else {
                let item = &verified.manifest.components[index - 2];
                let mut input = open_regular(&payload, &item.filename, item.bytes)?;
                let input_id = native::file_identity(&input)?;
                let before = input.metadata()?;
                let mut remaining = item.bytes;
                let mut hash = Sha256::new();
                let mut buffer = [0u8; 64 * 1024];
                while remaining != 0 {
                    let limit = remaining.min(buffer.len() as u64) as usize;
                    let read = input.read(&mut buffer[..limit])?;
                    if read == 0 {
                        return Err(Error::Payload);
                    }
                    output.write_all(&buffer[..read])?;
                    hash.update(&buffer[..read]);
                    remaining -= read as u64;
                }
                let mut excess = [0];
                let after = input.metadata()?;
                if input.read(&mut excess)? != 0
                    || before.len() != item.bytes
                    || after.len() != before.len()
                    || after.modified()? != before.modified()?
                    || native::file_identity(&input)? != input_id
                    || native::file_identity(&open_regular(&payload, &item.filename, item.bytes)?)?
                        != input_id
                    || hex(&hash.finalize()) != item.sha256
                {
                    return Err(Error::Payload);
                }
                (item.bytes, item.sha256.clone())
            };
            output.sync_all()?;
            if native::file_identity(&output)? != id {
                return Err(Error::Staging);
            }
            // Windows cannot reopen with deny-write sharing while our writer is
            // still live. Close it, reopen in the protected directory, then bind
            // the original full native ID and independently rehash all bytes.
            drop(output);
            let file = native::pin_file(&self.directory, &name)?;
            if native::file_identity(&file)? != id {
                return Err(Error::Staging);
            }
            let mut item = PinnedFile {
                name,
                file,
                id,
                bytes,
                sha256: digest,
            };
            item.verify(&self.directory)?;
            pinned.push(item);
        }
        if native::directory_identity(source)? != source_id
            || native::directory_identity(&source.open_dir_nofollow("payload")?)? != payload_id
        {
            return Err(Error::Payload);
        }
        exact_entries(source, &["manifest.json", "manifest.sig.json", "payload"])?;
        exact_entries(&payload, &names)?;
        native::sync_directory(&self.directory)?;
        let mut staged = StagedPackage {
            area: self,
            verified,
            files: pinned,
        };
        staged.verify()?;
        Ok(staged)
    }
}

impl StagedPackage {
    /// Build every field from this exact, freshly rechecked package. An earlier
    /// independent source inspection is never the basis of a staging receipt.
    pub fn receipt(&mut self) -> Result<StagingReceipt<'_>> {
        self.verify()?;
        Ok(StagingReceipt {
            manifest: self.manifest(),
            manifest_sha256: self.manifest_sha256(),
            signature_verified: true,
            payload_verified: true,
            staged: true,
            installed: false,
            qualified: false,
            execution_authority: false,
            native_custody_reconstructible: false,
        })
    }

    pub fn manifest(&self) -> &PackageManifest {
        self.verified.manifest()
    }
    pub fn manifest_sha256(&self) -> &str {
        self.verified.manifest_sha256()
    }

    /// Rechecks the original directory and every retained original file. This
    /// never reopens a serialized capability, installs or admits execution.
    pub fn verify(&mut self) -> Result<()> {
        self.area.check()?;
        let names: Vec<_> = self.files.iter().map(|f| f.name.as_str()).collect();
        exact_entries(&self.area.directory, &names)?;
        for file in &mut self.files {
            file.verify(&self.area.directory)?;
        }
        self.area.check()
    }
}

impl PinnedFile {
    fn verify(&mut self, directory: &Dir) -> Result<()> {
        if native::file_identity(&self.file)? != self.id
            || native::file_identity(&native::pin_file(directory, &self.name)?)? != self.id
        {
            return Err(Error::Staging);
        }
        native::check_allocation(&self.file, self.bytes)?;
        self.file.seek(SeekFrom::Start(0))?;
        let mut hash = Sha256::new();
        let mut remaining = self.bytes;
        let mut buffer = [0u8; 64 * 1024];
        while remaining != 0 {
            let limit = remaining.min(buffer.len() as u64) as usize;
            let n = self.file.read(&mut buffer[..limit])?;
            if n == 0 {
                return Err(Error::Payload);
            }
            hash.update(&buffer[..n]);
            remaining -= n as u64;
        }
        let mut excess = [0];
        if self.file.read(&mut excess)? != 0
            || hex(&hash.finalize()) != self.sha256
            || native::file_identity(&self.file)? != self.id
        {
            return Err(Error::Payload);
        }
        self.file.seek(SeekFrom::Start(0))?;
        Ok(())
    }
}
