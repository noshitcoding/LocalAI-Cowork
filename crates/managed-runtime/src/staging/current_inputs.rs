//! Private fresh installed-input acquisition. This never opens a publication
//! attempt or recovers its dispatch/cleanup authority. The enclosing fresh
//! installation loader alone owns the resulting current descriptors.
use super::*;

impl StagingArea {
    pub(super) fn open_current(parent: Dir, name: &str, payload_limit: u64) -> Result<Self> {
        if unsafe { libc::geteuid() } != 0
            || !token(name)
            || payload_limit == 0
            || payload_limit > MAX_PACKAGE
        {
            return Err(Error::Staging);
        }
        native::private_directory(&parent)?;
        let parent_id = native::directory_identity(&parent)?;
        let directory = parent.open_dir_nofollow(name)?;
        native::private_directory(&directory)?;
        let directory_id = native::directory_identity(&directory)?;
        let value = Self {
            parent,
            directory,
            name: name.into(),
            parent_id,
            directory_id,
            payload_limit,
        };
        value.check()?;
        Ok(value)
    }
}

impl StagedPackage {
    /// Internal content reuse only. This is fresh verification of all current
    /// signed bytes, not reconstruction from a receipt or stored local IDs.
    pub(super) fn open_current_inputs(
        parent: Dir,
        trust: &TrustRoot,
        platform: Platform,
        expected_manifest: &str,
        payload_limit: u64,
    ) -> Result<Self> {
        let area = StagingArea::open_current(parent, "package", payload_limit)?;
        let manifest = read_small(&area.directory, "manifest.json", MAX_MANIFEST)?;
        let signature = read_small(&area.directory, "manifest.sig.json", 2048)?;
        let verified = verify_manifest(&manifest, &signature, trust, platform)?;
        if verified.manifest_sha256() != expected_manifest {
            return Err(Error::Archive("installed_package_identity"));
        }
        let mut expected = vec![
            (
                "manifest.json".to_owned(),
                manifest.len() as u64,
                sha256(&manifest),
            ),
            (
                "manifest.sig.json".to_owned(),
                signature.len() as u64,
                sha256(&signature),
            ),
        ];
        let mut total = 0u64;
        for (i, component) in verified.manifest().components.iter().enumerate() {
            total = total.checked_add(component.bytes).ok_or(Error::Staging)?;
            expected.push((
                format!("component-{i}.bin"),
                component.bytes,
                component.sha256.clone(),
            ));
        }
        if total > payload_limit {
            return Err(Error::Archive("installed_input_budget"));
        }
        let names: Vec<_> = expected.iter().map(|x| x.0.as_str()).collect();
        exact_entries(&area.directory, &names)?;
        let mut files = Vec::with_capacity(expected.len());
        for (name, bytes, sha256) in expected {
            let file = native::pin_file(&area.directory, &name)?;
            let id = native::file_identity(&file)?;
            let mut original = PinnedFile {
                name,
                file,
                id,
                bytes,
                sha256,
            };
            original.verify(&area.directory)?;
            files.push(original);
        }
        let mut value = Self {
            area,
            verified,
            files,
        };
        value.verify()?;
        Ok(value)
    }
}
