use cowork_workflow_contracts::*;
use serde_json::json;
#[test]
fn human_answer_commands_never_accept_browser_authority() {
    let draft = json!({"fingerprint":"a".repeat(64),"base_revision":0,"idempotency_key":"save","answer":"  incomplete  "});
    let prepare =
        json!({"fingerprint":"a".repeat(64),"base_draft_revision":1,"idempotency_key":"prepare"});
    assert!(serde_json::from_value::<SaveWorkflowInputDraft>(draft.clone()).is_ok());
    assert!(serde_json::from_value::<PrepareWorkflowInputAnswer>(prepare.clone()).is_ok());
    for field in [
        "owner",
        "project_id",
        "run_id",
        "request_id",
        "receipt",
        "answer",
        "state",
        "grant",
        "spec",
    ] {
        if field != "answer" {
            let mut forged = draft.clone();
            forged[field] = json!("forged");
            assert!(
                serde_json::from_value::<SaveWorkflowInputDraft>(forged).is_err(),
                "draft accepted {field}"
            );
        }
        let mut forged = prepare.clone();
        forged[field] = json!("forged");
        assert!(
            serde_json::from_value::<PrepareWorkflowInputAnswer>(forged).is_err(),
            "prepare accepted {field}"
        );
    }
}
#[test]
fn exact_answer_survives_wire_roundtrip_and_envelope_is_strict() {
    let text = "  ö😀\r\n\"quoted\"\\path\n  ";
    let body = json!({"input":{"request_id":"question","answer":text},"fingerprint":"a".repeat(64),"idempotency_key":"key"});
    let decoded: SubmitWorkflowInput = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(decoded.input.answer.as_bytes(), text.as_bytes());
    assert_eq!(serde_json::to_value(decoded).unwrap(), body);
    let mut forged = body;
    forged["owner"] = json!("other");
    assert!(serde_json::from_value::<SubmitWorkflowInput>(forged).is_err());
}
#[test]
fn aggregate_exposes_input_durability_without_dispatch_authority() {
    let schema = serde_json::to_value(schemars::schema_for!(ApiContractV2)).unwrap();
    for name in [
        "WorkflowInputView",
        "WorkflowInputDraftLookup",
        "WorkflowInputDraftSnapshot",
        "WorkflowInputAnswerIntent",
        "PrepareWorkflowInputAnswer",
        "WorkflowInputAnswerExport",
        "WorkflowInputResumePreview",
        "ResumeWorkflowInput",
    ] {
        assert!(schema["$defs"].get(name).is_some(), "missing {name}");
    }
    for name in [
        "SaveWorkflowInputDraft",
        "PrepareWorkflowInputAnswer",
        "WorkflowInputAnswerIntent",
    ] {
        assert_eq!(schema["$defs"][name]["additionalProperties"], false);
    }
}

#[test]
fn resume_expectations_never_carry_an_answer_or_execution_authority() {
    let body = json!({"run_id":"run","base_run_revision":4,"base_workflow_revision":3,
        "expected_checkpoint_sha256":"c".repeat(64),"expected_cursor_sha256":"d".repeat(64),"idempotency_key":"resume"});
    let request: ResumeWorkflowInput = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), body);
    for field in [
        "answer",
        "owner",
        "execution_claim",
        "lease",
        "generation",
        "spec",
        "grant",
        "budget",
        "created_at",
    ] {
        let mut forged = body.clone();
        forged[field] = json!("authority");
        assert!(
            serde_json::from_value::<ResumeWorkflowInput>(forged).is_err(),
            "accepted {field}"
        );
    }
}
