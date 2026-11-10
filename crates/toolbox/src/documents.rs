use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentVersion {
    pub id: String,
    pub document_id: String,
    pub revision: u32,
    pub path: String,
    pub file_name: String,
    pub sha256: String,
    pub size: u64,
    pub created_at: u64,
    pub operation: String,
    pub job_id: String,
    pub parent_sha256: Option<String>,
}
impl Toolbox {
    fn document_directory(&self, order: &str) -> Result<PathBuf> {
        identifier(order)?;
        let directory = self.config.root.join("documents").join(order);
        private_dir(&directory)?;
        Ok(directory)
    }
    fn all_document_versions(&self, order: &str) -> Result<Vec<DocumentVersion>> {
        let directory = self.document_directory(order)?;
        let mut versions = Vec::new();
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.path().extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            namespace(&entry.path())?;
            let mut file = OpenOptions::new().read(true).open(entry.path())?;
            single_file(&file)?;
            if file.metadata()?.len() > 16000 || versions.len() >= 1000 {
                return Err(invalid("Document history exceeds its supported bounds"));
            }
            let mut raw = Vec::new();
            file.read_to_end(&mut raw)?;
            let value: DocumentVersion = serde_json::from_slice(&raw)?;
            identifier(&value.id)?;
            identifier(&value.document_id)?;
            if entry.path().file_stem().and_then(|s| s.to_str()) != Some(value.id.as_str()) {
                return Err(Error::Conflict);
            }
            versions.push(value);
        }
        versions.sort_by_key(|version| (version.revision, version.created_at, version.id.clone()));
        Ok(versions)
    }
    pub fn document_versions(&self, order: &str, path: &str) -> Result<Vec<DocumentVersion>> {
        relative(path)?;
        let _lease = self.workspace_lease(order)?;
        let all = self.all_document_versions(order)?;
        let document = all
            .iter()
            .rev()
            .find(|version| version.path == path)
            .map(|version| version.document_id.clone());
        Ok(all
            .into_iter()
            .filter(|version| Some(&version.document_id) == document.as_ref())
            .collect())
    }
    pub fn read_document_version(
        &self,
        order: &str,
        id: &str,
    ) -> Result<(DocumentVersion, Vec<u8>)> {
        identifier(id)?;
        let _lease = self.workspace_lease(order)?;
        let version = self
            .all_document_versions(order)?
            .into_iter()
            .find(|version| version.id == id)
            .ok_or(Error::NotFound)?;
        let path = self.document_directory(order)?.join(format!("{id}.bin"));
        namespace(&path)?;
        let file = OpenOptions::new().read(true).open(path)?;
        single_file(&file)?;
        if file.metadata()?.len() != version.size || version.size > 32 * 1024 * 1024 {
            return Err(Error::Conflict);
        }
        let mut bytes = Vec::new();
        (&file).take(version.size + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != version.size || hash(&bytes) != version.sha256 {
            return Err(Error::Conflict);
        }
        Ok((version, bytes))
    }
    fn snapshot_document(
        &self,
        order: &str,
        path: &str,
        bytes: &[u8],
        operation: &str,
        job: &str,
        parent: Option<(&str, &str)>,
    ) -> Result<DocumentVersion> {
        let mut all = self.all_document_versions(order)?;
        if let Some(existing) = all
            .iter()
            .find(|version| version.job_id == job && version.path == path)
        {
            if existing.sha256 != hash(bytes) {
                return Err(Error::Conflict);
            }
            return Ok(existing.clone());
        }
        if all.len() >= 1000
            || all.iter().map(|version| version.size).sum::<u64>() + bytes.len() as u64
                > 512 * 1024 * 1024
        {
            return Err(invalid("Document revision quota reached"));
        }
        let source = parent
            .and_then(|(path, sha)| {
                all.iter()
                    .rev()
                    .find(|version| version.path == path && version.sha256 == sha)
            })
            .cloned();
        let document_id = source
            .as_ref()
            .map(|version| version.document_id.clone())
            .or_else(|| {
                all.iter()
                    .rev()
                    .find(|version| version.path == path)
                    .map(|version| version.document_id.clone())
            })
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let revision = all
            .iter()
            .filter(|version| version.document_id == document_id)
            .map(|version| version.revision)
            .max()
            .unwrap_or(0)
            + 1;
        let version = DocumentVersion {
            id: Uuid::new_v4().to_string(),
            document_id,
            revision,
            path: path.into(),
            file_name: Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid("Invalid document filename"))?
                .into(),
            sha256: hash(bytes),
            size: bytes.len() as u64,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid("Clock unavailable"))?
                .as_millis() as u64,
            operation: operation.into(),
            job_id: job.into(),
            parent_sha256: parent.map(|(_, sha)| sha.into()),
        };
        let directory = self.document_directory(order)?;
        let mut binary = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(format!("{}.bin", version.id)))?;
        binary.write_all(bytes)?;
        binary.sync_all()?;
        single_file(&binary)?;
        drop(binary);
        let mut metadata = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(format!("{}.json", version.id)))?;
        metadata.write_all(&serde_json::to_vec(&version)?)?;
        metadata.sync_all()?;
        all.push(version.clone());
        Ok(version)
    }
    pub(crate) fn snapshot_uploaded_document(
        &self,
        order: &str,
        path: &str,
        bytes: &[u8],
    ) -> Result<DocumentVersion> {
        self.snapshot_document(
            order,
            path,
            bytes,
            "upload",
            &format!("upload:{}", hash(bytes)),
            None,
        )
    }
    pub(crate) fn snapshot_office_result(
        &self,
        order: &str,
        request: &Request,
        job: &str,
        outcome: &cowork_executor::Outcome,
        lease: Arc<WorkspaceLease>,
    ) -> Result<()> {
        let Action::Office {
            operation,
            input,
            output,
            ..
        } = &request.action
        else {
            return Ok(());
        };
        if outcome.exit_code != Some(0) || outcome.stdout.truncated || !outcome.output_complete {
            return Ok(());
        }
        if output.is_none() && operation != "snapshot" {
            return Ok(());
        }
        let result: serde_json::Value = serde_json::from_slice(&outcome.stdout.bytes)?;
        let mut parent = None;
        if let Some(input) = input {
            let bytes = self.read_output_file_with_lease(order, input, lease.clone())?;
            let sha = hash(&bytes);
            if result["source_sha256"].as_str() != Some(sha.as_str()) {
                return Err(Error::Conflict);
            }
            if !self
                .all_document_versions(order)?
                .iter()
                .any(|version| version.path == *input && version.sha256 == sha)
            {
                self.snapshot_document(
                    order,
                    input,
                    &bytes,
                    "source_snapshot",
                    &format!("{job}:source"),
                    None,
                )?;
            }
            parent = Some((input.clone(), sha));
        }
        if let Some(output) = output {
            let bytes = self.read_output_file_with_lease(order, output, lease)?;
            let sha = hash(&bytes);
            if result["output_sha256"].as_str() != Some(sha.as_str()) {
                return Err(Error::Conflict);
            }
            self.snapshot_document(
                order,
                output,
                &bytes,
                operation,
                job,
                parent
                    .as_ref()
                    .map(|(path, sha)| (path.as_str(), sha.as_str())),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_versions_are_immutable_reopenable_scoped_and_integrity_checked() {
        let root = std::env::temp_dir().join(format!("cowork-document-test-{}", Uuid::new_v4()));
        let config = || Config {
            root: root.clone(),
            programs: BTreeMap::new(),
            environment: BTreeMap::new(),
            github_token: None,
        };
        let toolbox = Toolbox::open(config()).unwrap();
        let order = Uuid::new_v4().to_string();
        toolbox
            .write_input_file(&order, "documents/one.txt", b"first revision")
            .unwrap();
        let first = toolbox
            .document_versions(&order, "documents/one.txt")
            .unwrap()
            .pop()
            .unwrap();
        let second = toolbox
            .snapshot_document(
                &order,
                "documents/two.txt",
                b"second revision",
                "edit_docx",
                "job-two",
                Some(("documents/one.txt", &first.sha256)),
            )
            .unwrap();
        assert_eq!(second.document_id, first.document_id);
        assert_eq!(second.revision, 2);
        assert_eq!(
            toolbox
                .document_versions(&order, "documents/two.txt")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            toolbox
                .snapshot_document(
                    &order,
                    "documents/two.txt",
                    b"second revision",
                    "edit_docx",
                    "job-two",
                    None
                )
                .unwrap()
                .id,
            second.id
        );
        assert!(
            toolbox
                .snapshot_document(
                    &order,
                    "documents/two.txt",
                    b"different",
                    "edit_docx",
                    "job-two",
                    None
                )
                .is_err()
        );
        fs::write(
            toolbox.workspace(&order).unwrap().join("documents/one.txt"),
            b"current changed",
        )
        .unwrap();
        drop(toolbox);
        let toolbox = Toolbox::open(config()).unwrap();
        assert_eq!(
            toolbox.read_document_version(&order, &first.id).unwrap().1,
            b"first revision"
        );
        assert!(
            toolbox
                .read_document_version(&Uuid::new_v4().to_string(), &first.id)
                .is_err()
        );
        fs::write(
            root.join("documents")
                .join(&order)
                .join(format!("{}.bin", first.id)),
            b"tampered",
        )
        .unwrap();
        assert!(toolbox.read_document_version(&order, &first.id).is_err());
        drop(toolbox);
        fs::remove_dir_all(root).unwrap();
    }
}
