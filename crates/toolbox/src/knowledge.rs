use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeExtraction {
    pub format: String,
    pub source_sha256: String,
    pub text_sha256: String,
    pub text: String,
    pub method: String,
}
impl Toolbox {
    /// Explicit owner upload authorizes only this fixed, read-only parser. It
    /// does not grant arbitrary native Office actions or model tool authority.
    pub fn extract_knowledge(
        self: &Arc<Self>,
        project: &str,
        operation: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<KnowledgeExtraction> {
        if project.is_empty() || project.len() > 200 || project.chars().any(char::is_control) {
            return Err(invalid("Invalid extraction project"));
        }
        identifier(operation)?;
        let extension = Path::new(name)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.len() > 200
            || name.contains(['\0', '\r', '\n', '/', '\\', ':'])
            || !["pdf", "docx", "xlsx", "pptx"].contains(&extension.as_str())
            || bytes.is_empty()
            || bytes.len() > 16 * 1024 * 1024
        {
            return Err(invalid("Invalid document extraction input"));
        }
        let digest =
            Sha256::digest(format!("knowledge-extract/1:{project}:{operation}").as_bytes());
        let mut identity = [0u8; 16];
        identity.copy_from_slice(&digest[..16]);
        let order = Uuid::from_bytes(identity).to_string();
        let input = format!("source.{extension}");
        let sha = hash(bytes);
        if self.workspace(&order)?.join(&input).exists() {
            if hash(&self.read_output_file(&order, &input)?) != sha {
                return Err(Error::Conflict);
            }
        } else {
            self.write_input_file(&order, &input, bytes)?;
        }
        let mut request = Request {
            idempotency_key: operation.into(),
            action: Action::Office {
                operation: "extract_knowledge".into(),
                input: Some(input),
                output: None,
                text: Some(serde_json::json!({"source_sha256":sha}).to_string()),
                title: Some(name.into()),
            },
            timeout_seconds: 90,
            output_bytes: 1024 * 1024,
            approval_sha256: None,
        };
        request.approval_sha256 = Some(self.preview(&order, &request)?.request_sha256);
        let mut job = self.submit(&order, request)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(100);
        while job.state == "running" {
            if std::time::Instant::now() >= deadline {
                let _ = self.cancel(&order, &job.id);
                return Err(invalid(
                    "Document extraction outcome is unconfirmed; original operation must be reconciled",
                ));
            }
            std::thread::sleep(Duration::from_millis(40));
            job = self.status(&order, &job.id)?;
        }
        let outcome=job.outcome.as_ref().filter(|outcome|job.state=="completed"&&outcome.exit_code==Some(0)&&outcome.output_complete&&!outcome.stdout.truncated).ok_or_else(||invalid("Document extraction failed; check format, parser runtime and document size limits"))?;
        let result: KnowledgeExtraction = serde_json::from_slice(&outcome.stdout.bytes)?;
        if result.source_sha256 != sha
            || result.text_sha256 != hash(result.text.as_bytes())
            || result.format != extension
            || result.text.len() > 400000
            || result.text.trim().is_empty()
            || result.text.contains('\0')
            || result.method != "bounded-office-text/1"
        {
            return Err(Error::Conflict);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_knowledge_extraction_keeps_unicode_and_literal_formulas() {
        let Some(python) = std::env::var_os("COWORK_TEST_DOCUMENT_PYTHON") else {
            assert_ne!(
                std::env::var("COWORK_REQUIRE_DOCUMENT_TESTS").as_deref(),
                Ok("1"),
                "COWORK_TEST_DOCUMENT_PYTHON is required"
            );
            return;
        };
        let python = PathBuf::from(python);
        struct Scratch(PathBuf);
        impl Scratch {
            fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for Scratch {
            fn drop(&mut self) {
                if std::env::var("COWORK_KEEP_STAGES").as_deref() == Ok("1") {
                    return;
                }
                // Only the exact fresh directory directly below TEMP is eligible.
                if let (Ok(path), Ok(parent)) =
                    (self.0.canonicalize(), std::env::temp_dir().canonicalize())
                    && path.parent() == Some(parent.as_path())
                {
                    let _ = fs::remove_dir_all(&path);
                }
            }
        }
        let scratch =
            Scratch(std::env::temp_dir().join(format!("cowork-knowledge-{}", Uuid::new_v4())));
        fs::create_dir(scratch.path()).unwrap();
        #[cfg(windows)]
        {
            let canonical = scratch.path().canonicalize().unwrap();
            let conventional = python_working_directory(&canonical).unwrap();
            assert!(!conventional.to_string_lossy().starts_with(r"\\?\"));
            assert_eq!(conventional.canonicalize().unwrap(), canonical);
            let ordinary = scratch.path().join("alias");
            fs::create_dir(&ordinary).unwrap();
            let distinct =
                PathBuf::from(format!("{}.", ordinary.canonicalize().unwrap().display()));
            fs::create_dir(&distinct).unwrap();
            assert!(
                python_working_directory(&distinct).is_err(),
                "trailing-dot alias must not change the directory"
            );
        }
        let source = scratch.path().join("source.xlsx");
        let environment: BTreeMap<String, String> = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
            .collect();
        let launch = cowork_executor::TrustedLaunch {
            program: python.clone(), arguments: vec!["-I".into(), "-X".into(), "utf8".into(), "-c".into(),
                "import sys,openpyxl; b=openpyxl.Workbook(); b.active['A1']='Gr\\u00fc\\u00dfe'; b.active['B1']='=1+1'; b.save(sys.argv[1])".into(),
                source.to_string_lossy().into()], directory: scratch.path().into(), environment: environment.clone(),
            timeout: Duration::from_secs(30), capture_bytes_per_stream: 4096,
        };
        let generated = cowork_executor::supervise(&launch, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            generated.exit_code,
            Some(0),
            "{}",
            String::from_utf8_lossy(&generated.stderr.bytes)
        );
        let bytes = fs::read(source).unwrap();
        let toolbox = Toolbox::open(Config {
            root: scratch.path().join("toolbox"),
            programs: BTreeMap::from([("python".into(), python)]),
            environment,
            github_token: None,
        })
        .unwrap();
        // Exercise the real native supervisor and its isolated UTF-8 parser,
        // including the middot in the worksheet locator and a non-ASCII cell value.
        let result = toolbox.extract_knowledge(
            "project",
            &Uuid::new_v4().to_string(),
            "source.xlsx",
            &bytes,
        );
        assert!(
            result.is_ok(),
            "{result:?}; native outcomes: {:?}; fixture: {:?}",
            toolbox
                .jobs
                .lock()
                .unwrap()
                .values()
                .map(|job| &job.job)
                .collect::<Vec<_>>(),
            scratch.path()
        );
        let result = result.unwrap();
        assert!(result.text.contains("Grüße"), "{}", result.text);
        assert!(result.text.contains(" · Row 1"), "{}", result.text);
        assert!(result.text.contains("=1+1"));
        assert_eq!(result.source_sha256, hash(&bytes));
        assert_eq!(result.text_sha256, hash(result.text.as_bytes()));
        toolbox.shutdown();
    }
}
