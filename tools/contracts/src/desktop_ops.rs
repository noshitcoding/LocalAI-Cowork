use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output, array) in [
        ("/terminals/status", "get", "", "OwnerTerminalStatus", false),
        (
            "/orders/{order}/terminals",
            "get",
            "",
            "OwnerTerminalSession",
            true,
        ),
        (
            "/orders/{order}/terminals",
            "post",
            "OwnerTerminalLaunch",
            "OwnerTerminalSession",
            false,
        ),
        (
            "/orders/{order}/terminals/preview",
            "post",
            "OwnerTerminalLaunch",
            "OwnerTerminalPreview",
            false,
        ),
        (
            "/orders/{order}/terminals/{session}/output",
            "get",
            "",
            "OwnerTerminalOutput",
            false,
        ),
        (
            "/orders/{order}/terminals/{session}/input",
            "post",
            "OwnerTerminalInput",
            "OwnerTerminalSession",
            false,
        ),
        (
            "/orders/{order}/terminals/{session}/resize",
            "post",
            "OwnerTerminalResize",
            "OwnerTerminalSession",
            false,
        ),
        (
            "/orders/{order}/terminals/{session}/stop",
            "post",
            "",
            "OwnerTerminalSession",
            false,
        ),
        ("/maintenance", "get", "", "MaintenanceStatus", false),
        (
            "/maintenance/backups",
            "post",
            "OwnerDatabaseBackupRequest",
            "DatabaseBackup",
            false,
        ),
    ] {
        let schema = if array {
            json!({"type":"array","items":{"$ref":format!("#/components/schemas/{output}")}})
        } else {
            json!({"$ref":format!("#/components/schemas/{output}")})
        };
        let mut operation = json!({"security":[{"daemonBearer":[]}],"description":"Installation-owner operation only; actor/session tokens never authorize host processes or installation backups. Terminal starts require the exact reviewed approval digest. PTY input and output are not persisted by the daemon.","responses":{"200":{"description":"Original operation or current session state","content":{"application/json":{"schema":schema}}},"400":{"description":"Invalid input or unavailable configured capability"},"401":{"description":"Owner bearer required"},"403":{"description":"Origin or resource boundary rejected"},"404":{"description":"Resource absent in this chat"},"409":{"description":"Original operation pending or conflicting sequence"}}});
        if !input.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        let mut params: Vec<Value> = ["order", "session"]
            .iter()
            .filter(|name| path.contains(&format!("{{{name}}}")))
            .map(|name| json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}))
            .collect();
        if path.ends_with("/output") {
            params.push(json!({"name":"after","in":"query","schema":{"type":"integer","minimum":0,"default":0}}));
        }
        if !params.is_empty() {
            operation["parameters"] = json!(params);
        }
        document["paths"][path][method] = operation;
    }
    document["paths"]["/maintenance/cleanup"]["post"] = json!({"security":[{"daemonBearer":[]}],"description":"Apply saved inactive-owner-chat archive retention; immutable audit/history and actor content remain intact.","responses":{"200":{"description":"Archived owner chats","content":{"application/json":{"schema":{"type":"object","properties":{"archived_chats":{"type":"integer","minimum":0}},"required":["archived_chats"]}}}}}});
    document["paths"]["/maintenance/backups/{id}"]["get"] = json!({"security":[{"daemonBearer":[]}],"parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}],"description":"Owner-only immutable database image; verifies stored byte count and digest before streaming. Content-Length, X-Cowork-Sha256 and X-Cowork-File-Name identify the same bytes.","responses":{"200":{"description":"Verified database image","content":{"application/octet-stream":{"schema":{"type":"string","format":"binary"}}}}}});
}
