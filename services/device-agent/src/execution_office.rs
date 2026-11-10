//! File-pinned Office operations. Working copies are private; publication is no-replace.
use super::*;
use cowork_contracts::DeviceOfficeRequest;
use std::io::Read;
pub(super) fn definitions() -> Vec<Value> {
    vec![
        definition(
            "microsoft_office",
            "Use newly owned Word/Excel/PowerPoint on a private desktop. Macros, active content, links and foreign instances are forbidden. Source is unchanged; outputs must be new artifacts/ paths.",
            json!({"application":{"type":"string","enum":["word","excel","powerpoint"]},"action":{"type":"string","enum":["export_pdf","replace_text","word_append_paragraph","word_format_text","excel_set_cell","excel_format_range","excel_add_chart","powerpoint_add_slide","powerpoint_format_text"]},"source":{"type":"string"},"output":{"type":"string"},"preview_output":{"type":"string"},"parameters":{"type":"object"}}),
            &["application", "source", "output"],
        ),
        definition(
            "office_workflow",
            "Generate structured DOCX/PPTX/XLSX/PDF, transform an exact DOCX/PPTX template, or produce native and template outputs together.",
            json!({"format":{"type":"string","enum":["docx","pptx","xlsx","pdf"]},"output_path":{"type":"string"},"mode":{"type":"string","enum":["native","template","parallel"]},"template_path":{"type":"string"},"title":{"type":"string"},"sections":{"type":"array","items":{"type":"object"}},"sections_json":{"type":"string"},"paragraphs":{"type":"array","items":{"type":"string"}},"bullets":{"type":"array","items":{"type":"string"}},"transforms":{"type":"object"}}),
            &["format", "output_path"],
        ),
    ]
}
fn suffix(path: &str) -> Result<&str> {
    path.rsplit_once('.')
        .map(|(_, s)| s)
        .context("device_office_extension_missing")
}
fn held_program(path: &Path) -> Result<fs::File> {
    crate::config::ordinary(path, false)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000);
    }
    Ok(options.open(path)?)
}
fn validate(r: &DeviceOfficeRequest) -> Result<()> {
    let actions: &[&str] = match r.application.as_str() {
        "word" => &[
            "export_pdf",
            "replace_text",
            "word_append_paragraph",
            "word_format_text",
        ],
        "excel" => &[
            "export_pdf",
            "replace_text",
            "excel_set_cell",
            "excel_format_range",
            "excel_add_chart",
        ],
        "powerpoint" => &[
            "export_pdf",
            "replace_text",
            "powerpoint_add_slide",
            "powerpoint_format_text",
        ],
        _ => bail!("device_office_application_invalid"),
    };
    ensure!(
        actions.contains(&r.action.as_str())
            && r.parameters.is_object()
            && serde_json::to_vec(&r.parameters)?.len() <= 1024 * 1024,
        "device_office_action_invalid"
    );
    let ext = suffix(&r.source)?.to_ascii_lowercase();
    let expected = match r.application.as_str() {
        "word" => ["doc", "docx"],
        "excel" => ["xls", "xlsx"],
        _ => ["ppt", "pptx"],
    };
    ensure!(
        expected.contains(&ext.as_str()),
        "device_office_source_format_invalid"
    );
    ensure!(
        r.output.starts_with("artifacts/")
            && suffix(&r.output)?
                == if r.action == "export_pdf" {
                    "pdf"
                } else {
                    expected[1]
                },
        "device_office_output_format_invalid"
    );
    if let Some(preview) = &r.preview_output {
        ensure!(
            r.action != "export_pdf"
                && preview.starts_with("artifacts/")
                && suffix(preview)? == "pdf"
                && preview != &r.output,
            "device_office_preview_invalid"
        );
        workspace::relative(preview)?;
    }
    workspace::relative(&r.source)?;
    workspace::relative(&r.output)?;
    for key in ["cell", "range"] {
        if let Some(v) = r.parameters[key].as_str() {
            ensure!(
                !v.is_empty()
                    && v.len() <= 200
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_:$,.".contains(&b)),
                "device_office_external_range"
            );
        }
    }
    if let Some(formula) = r.parameters["formula"].as_str() {
        ensure!(
            formula.len() <= 8192
                && formula.starts_with('=')
                && !formula.contains(['!', '[', ']', '\\', '|', '@'])
                && !formula.contains("://"),
            "device_office_formula_external_or_active"
        );
        let upper = formula.to_ascii_uppercase();
        for active in [
            "WEBSERVICE",
            "HYPERLINK",
            "RTD",
            "CALL(",
            "REGISTER",
            "DDE",
            "EXEC",
            "SQL.REQUEST",
            "INDIRECT",
            "EVALUATE",
            "EXEC",
            "IMAGE(",
            "FILTERXML",
            "STOCKHISTORY",
        ] {
            ensure!(
                !upper.contains(active),
                "device_office_formula_external_or_active"
            );
        }
    }
    Ok(())
}
pub(super) fn publish(
    engine: &ExecutionEngine,
    run: Uuid,
    directory: &Path,
    file: &str,
    target: &str,
    expected: &Value,
) -> Result<Value> {
    ensure!(
        !file.contains(['/', '\\']) && file != "." && file != "..",
        "device_office_foreign_output"
    );
    let source = directory.join(file);
    crate::config::ordinary(&source, false)?;
    let mut original = held_program(&source)?;
    let metadata = original.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= 32 * 1024 * 1024,
        "device_office_original_output_bound"
    );
    let mut raw = vec![];
    Read::take(&mut original, 32 * 1024 * 1024 + 1).read_to_end(&mut raw)?;
    let after = original.metadata()?;
    ensure!(
        after.len() == metadata.len() && after.modified()? == metadata.modified()?,
        "device_office_original_output_changed"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = fs::metadata(&source)?;
        ensure!(
            current.dev() == after.dev() && current.ino() == after.ino() && current.nlink() == 1,
            "device_office_original_output_replaced"
        );
    }
    ensure!(
        raw.len() <= 32 * 1024 * 1024
            && expected["sha256"] == digest(&raw)
            && expected["bytes"].as_u64() == Some(raw.len() as u64),
        "device_office_output_binding_changed"
    );
    let root = engine
        .toolbox
        .workspace(&run.to_string())
        .map_err(|_| anyhow::anyhow!("device_workspace_unavailable"))?;
    let destination = root.join(workspace::relative(target)?);
    let parent = destination
        .parent()
        .context("device_office_parent_missing")?;
    private_directory(parent)?;
    crate::config::ordinary(parent, true)?;
    ensure!(
        parent.canonicalize()?.starts_with(&root),
        "device_office_destination_escape"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?;
    file.write_all(&raw)?;
    file.sync_all()?;
    drop(file);
    crate::config::ordinary(&destination, false)?;
    ensure!(
        digest(&fs::read(&destination)?) == digest(&raw),
        "device_office_publication_changed"
    );
    Ok(json!({"path":target,"sha256":digest(&raw),"bytes":raw.len()}))
}
#[expect(
    clippy::too_many_arguments,
    reason = "Office execution validates the named operation and its arguments against explicit owner rights, run lease, approval source and cancellation."
)]
pub(super) async fn call(
    engine: &ExecutionEngine,
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    source: Uuid,
    name: &str,
    args: Value,
    stop: &CancellationToken,
) -> Result<Value> {
    ensure!(
        engine.config.native_owner_rights
            && args.is_object()
            && serde_json::to_vec(&args)?.len() <= 2 * 1024 * 1024,
        "device_office_request_invalid"
    );
    let native = if name == "microsoft_office" {
        ensure!(
            cfg!(windows) && crate::desktop::available(),
            "device_microsoft_office_requires_unlocked_interactive_windows"
        );
        let r: DeviceOfficeRequest = serde_json::from_value(args.clone())?;
        validate(&r)?;
        Some(r)
    } else {
        None
    };
    if name == "office" && args["operation"] == "convert" {
        let input = args["input"]
            .as_str()
            .context("device_office_convert_source_missing")?;
        let output = args["output"]
            .as_str()
            .context("device_office_convert_output_missing")?;
        ensure!(
            ["doc", "docx", "xls", "xlsx", "ppt", "pptx", "pdf"]
                .contains(&suffix(input)?.to_ascii_lowercase().as_str())
                && ["docx", "xlsx", "pptx", "pdf", "txt", "csv"]
                    .contains(&suffix(output)?.to_ascii_lowercase().as_str()),
            "device_office_conversion_format_not_safe"
        );
    }
    let workspace_lease = engine
        .toolbox
        .workspace_lease(&run.to_string())
        .map_err(|_| anyhow::anyhow!("device_workspace_busy"))?;
    let d = engine
        .config
        .data_directory
        .join("interop")
        .join(run.to_string())
        .join(source.to_string());
    private_directory(&d)?;
    let adapters = engine.config.data_directory.join("interop-adapters");
    let helper = receipts::helper(
        &adapters,
        "office-documents.py",
        include_bytes!("device_office_documents.py"),
    )?;
    let policy = receipts::helper(
        &adapters,
        "document-policy.py",
        include_bytes!("device_document_policy.py"),
    )?;
    let python = engine
        .config
        .programs
        .get("python")
        .context("device_office_python_missing")?
        .clone();
    let input = native.as_ref().map(|r| r.source.as_str()).or_else(|| {
        if name == "office_workflow" {
            args["template_path"].as_str()
        } else {
            args["input"].as_str()
        }
    });
    let mut input_pin = Value::Null;
    let mut input_name = None;
    if let Some(input) = input {
        workspace::relative(input)?;
        let bytes = engine
            .toolbox
            .read_output_file_with_lease(&run.to_string(), input, workspace_lease.clone())
            .map_err(|_| anyhow::anyhow!("device_office_original_input_unavailable"))?;
        let ext = suffix(input)?.to_ascii_lowercase();
        let copied = format!("original.{ext}");
        let mut f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(d.join(&copied))?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        input_pin =
            json!({"path":input,"sha256":digest(&bytes),"bytes":bytes.len(),"extension":ext});
        input_name = Some(copied);
    }
    let format = args["format"]
        .as_str()
        .or_else(|| args["operation"].as_str())
        .unwrap_or("");
    let packages =
        if native.is_some() || args["mode"] == "template" || args["operation"] == "convert" {
            vec![]
        } else {
            match format {
                "docx" | "read_docx" | "create_docx" => vec!["python-docx"],
                "pptx" | "read_pptx" | "create_pptx" => vec!["python-pptx"],
                "xlsx" | "read_xlsx" | "create_xlsx" => vec!["openpyxl"],
                "pdf" | "create_pdf" => vec!["reportlab"],
                "read_pdf" => vec!["pypdf"],
                _ => vec![],
            }
        };
    let mut dependencies = Value::Null;
    if !packages.is_empty() {
        receipts::write(
            &d,
            "dependencies.json",
            &json!({"phase":"dependencies","policy_helper":policy,"packages":packages}),
        )?;
        dependencies = receipts::supervise(
            engine,
            run,
            python.clone(),
            vec![
                "-I".into(),
                helper.to_string_lossy().into(),
                d.to_string_lossy().into(),
                "dependencies.json".into(),
            ],
            d.clone(),
            30,
            stop,
        )
        .await?;
    }
    let mut proof = Value::Null;
    if let Some(input_name) = &input_name
        && (native.is_some() || name == "office_workflow" || args["operation"] == "convert")
    {
        let inspect = json!({"phase":"inspect","input_name":input_name,"extension":input_pin["extension"],"policy_helper":policy});
        receipts::write(&d, "inspect.json", &inspect)?;
        proof = receipts::supervise(
            engine,
            run,
            python.clone(),
            vec![
                "-I".into(),
                helper.to_string_lossy().into(),
                d.to_string_lossy().into(),
                "inspect.json".into(),
            ],
            d.clone(),
            30,
            stop,
        )
        .await?;
        ensure!(
            proof["policy"]["sha256"] == input_pin["sha256"],
            "device_office_original_policy_differs"
        );
    }
    let output = native
        .as_ref()
        .map(|r| r.output.as_str())
        .or_else(|| args["output_path"].as_str())
        .or_else(|| args["output"].as_str());
    let root = engine
        .toolbox
        .workspace(&run.to_string())
        .map_err(|_| anyhow::anyhow!("device_workspace_unavailable"))?;
    if let Some(path) = output {
        workspace::relative(path)?;
        ensure!(
            !root.join(path).exists(),
            "device_office_output_already_exists"
        );
    }
    if let Some(r) = &native
        && let Some(path) = &r.preview_output
    {
        ensure!(
            !root.join(path).exists(),
            "device_office_preview_already_exists"
        );
    }
    let mut program_pins = BTreeMap::new();
    let mut held_programs = vec![];
    for name in [
        Some("python"),
        native.as_ref().map(|r| r.application.as_str()),
        native.as_ref().map(|_| "powershell"),
        if args["operation"] == "convert" {
            Some("libreoffice")
        } else {
            None
        },
    ]
    .into_iter()
    .flatten()
    {
        let path = engine
            .config
            .programs
            .get(name)
            .context("device_office_program_missing")?;
        held_programs.push(held_program(path)?);
        program_pins.insert(name.to_owned(), digest(&fs::read(path)?));
    }
    let action = json!({"format":"device-original-office-plan/1","tool":name,"run_id":run,"source_request_id":source,"lease_sha256":digest(lease.as_bytes()),"arguments":args,"input":input_pin,"policy":proof,"dependencies":dependencies,"program_sha256":program_pins,"automatic_replay":false,"private_working_copy":true,"outputs_are_new":true});
    let sha = digest(&serde_json::to_vec(&action)?);
    receipts::write(&d, "original-plan.json", &action)?;
    if !engine
        .approval(control, run, lease, source, action.clone(), stop)
        .await?
    {
        return Ok(json!({"state":"rejected","performed":false}));
    }
    if let Some(input) = input {
        let bytes = engine
            .toolbox
            .read_output_file_with_lease(&run.to_string(), input, workspace_lease.clone())
            .map_err(|_| anyhow::anyhow!("device_office_original_input_changed"))?;
        ensure!(
            digest(&bytes) == input_pin["sha256"],
            "device_office_original_input_changed"
        );
    }
    for (name, pin) in &program_pins {
        let path = engine
            .config
            .programs
            .get(name)
            .context("device_office_program_missing")?;
        ensure!(
            digest(&fs::read(path)?) == *pin,
            "device_office_original_program_changed"
        );
    }
    if native.is_some() {
        ensure!(
            crate::desktop::available(),
            "device_office_interactive_session_changed"
        );
    }
    engine.admit(
        run,
        &source.to_string(),
        json!({"phase":"office_original_dispatched","request_sha256":sha,"action":action}),
    )?;
    engine
        .checkpoint(
            control,
            run,
            lease,
            source,
            json!({"tool":name,"request_sha256":sha}),
        )
        .await?;
    ensure!(!stop.is_cancelled(), "device_run_canceled");
    let mut native_receipt = Value::Null;
    let response = if let Some(r) = &native {
        let executable = engine
            .config
            .programs
            .get(&r.application)
            .context("device_office_program_not_selected")?;
        crate::config::ordinary(executable, false)?;
        let script = receipts::helper(
            &adapters,
            "owned-office.ps1",
            include_bytes!("office_windows.ps1"),
        )?;
        let request = json!({"application":r.application,"action":r.action,"parameters":r.parameters,"nonce":source.simple().to_string(),"request_sha256":sha,"source_sha256":input_pin["sha256"],"executable":executable,"program_sha256":digest(&fs::read(executable)?),"directory":d,"working_source":d.join(input_name.as_ref().context("device_office_source_missing")?),"working_output":d.join(format!("native.{}",suffix(&r.output)?)),"working_preview":r.preview_output.as_ref().map(|_|d.join("preview.pdf"))});
        receipts::write(&d, "native-request.json", &request)?;
        native_receipt = receipts::supervise(
            engine,
            run,
            engine
                .config
                .programs
                .get("powershell")
                .context("device_office_powershell_not_selected")?
                .clone(),
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-File".into(),
                script.to_string_lossy().into(),
                d.join("native-request.json").to_string_lossy().into(),
            ],
            d.clone(),
            600,
            stop,
        )
        .await?;
        ensure!(
            native_receipt["owned_job_empty"] == true
                && native_receipt["nonce"] == source.simple().to_string()
                && native_receipt["request_sha256"] == sha
                && native_receipt["source_sha256"] == input_pin["sha256"],
            "device_office_original_native_receipt_differs"
        );
        let mut outputs = vec![];
        for file in [
            Some(format!("native.{}", suffix(&r.output)?)),
            r.preview_output.as_ref().map(|_| "preview.pdf".into()),
        ]
        .into_iter()
        .flatten()
        {
            let req = json!({"phase":"inspect","input_name":file,"extension":suffix(&file)?,"policy_helper":policy});
            let request_name = format!("verify-{}.json", outputs.len());
            receipts::write(&d, &request_name, &req)?;
            let verified = receipts::supervise(
                engine,
                run,
                python.clone(),
                vec![
                    "-I".into(),
                    helper.to_string_lossy().into(),
                    d.to_string_lossy().into(),
                    request_name,
                ],
                d.clone(),
                30,
                stop,
            )
            .await?;
            outputs.push(json!({"file":file,"sha256":verified["policy"]["sha256"],"bytes":verified["policy"]["bytes"]}));
        }
        json!({"ok":true,"outputs":outputs})
    } else {
        let request = json!({"phase":if name=="office_workflow"{"workflow"}else{"generic"},"arguments":args,"input_name":input_name,"input_sha256":input_pin["sha256"],"policy_helper":policy,"office_helper":receipts::helper(&adapters,"generic-office.py",include_bytes!("../../../crates/toolbox/src/office.py"))?,"libreoffice":engine.config.programs.get("libreoffice")});
        receipts::write(&d, "generate.json", &request)?;
        receipts::supervise(
            engine,
            run,
            python.clone(),
            vec![
                "-I".into(),
                helper.to_string_lossy().into(),
                d.to_string_lossy().into(),
                "generate.json".into(),
            ],
            d.clone(),
            120,
            stop,
        )
        .await?
    };
    receipts::write(
        &d,
        "original-produced.json",
        &json!({"request_sha256":sha,"response":response,"native":native_receipt}),
    )?;
    let mut published = vec![];
    if let Some(outputs) = response["outputs"].as_array() {
        for (index, item) in outputs.iter().enumerate() {
            let target = if let Some(r) = &native {
                if index == 0 {
                    r.output.clone()
                } else {
                    r.preview_output
                        .clone()
                        .context("device_office_unexpected_preview")?
                }
            } else {
                let target = output.context("device_office_output_missing")?;
                if name == "office_workflow" && args["mode"] == "parallel" && index == 1 {
                    let (stem, ext) = target
                        .rsplit_once('.')
                        .context("device_office_output_extension")?;
                    format!("{stem}-template.{ext}")
                } else {
                    target.into()
                }
            };
            ensure!(!stop.is_cancelled(), "device_run_canceled");
            published.push(publish(
                engine,
                run,
                &d,
                item["file"]
                    .as_str()
                    .context("device_office_output_file_missing")?,
                &target,
                item,
            )?);
        }
    }
    let receipt = json!({"format":"device-original-office-receipt/1","run_id":run,"source_request_id":source,"request_sha256":sha,"source":input_pin,"result":response,"native":native_receipt,"published":published,"automatic_replay":false});
    receipts::write(&d, "original-receipt.json", &receipt)?;
    engine.admit(run, &format!("{source}/settled"), receipt.clone())?;
    Ok(engine.redact_value(receipt))
}
