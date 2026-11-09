use super::*;

impl FileBroker {
    pub fn validate_upload_metadata(file: &DownloadMetadata) -> Result<()> {
        valid_text(&file.file_name, 1024)?;
        if file.file_name.chars().any(char::is_control)
            || file.file_name.contains(['/', '\\'])
            || file.size > MAX_DOWNLOAD_BYTES
        {
            return Err(Error::Validation("Invalid upload metadata".into()));
        }
        validate_hash(&file.sha256)
    }

    /// Staging has no workspace side effect. Only the runtime can publish this retained blob.
    pub fn retain_upload(&self, upload: &FileUpload, bytes: &[u8]) -> Result<()> {
        Self::validate_upload_metadata(&upload.file)?;
        if bytes.len() as u64 != upload.file.size || hash(bytes) != upload.file.sha256 {
            return Err(Error::Validation("Upload integrity check failed".into()));
        }
        save_version(&self.versions, bytes)?;
        self.verify_upload(upload)
    }

    pub fn verify_upload(&self, upload: &FileUpload) -> Result<()> {
        Self::validate_upload_metadata(&upload.file)?;
        let bytes = read_regular_limit(&self.versions, &upload.file.sha256, MAX_DOWNLOAD_BYTES)?
            .ok_or(Error::NotFound)?;
        if bytes.len() as u64 != upload.file.size || hash(&bytes) != upload.file.sha256 {
            return Err(Error::Validation("Upload integrity check failed".into()));
        }
        Ok(())
    }
}
