//! Native and tenant M9 use the same canonical DTOs with separate current authorities.
use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    let endpoints = [
        (
            "/imports/{import}/content/catalog",
            "get",
            "",
            "HistoricalContentCatalogPage",
            false,
            true,
        ),
        (
            "/imports/{import}/content/catalog/{source}/{page}",
            "get",
            "",
            "binary",
            false,
            false,
        ),
        (
            "/imports/{import}/preview/paged",
            "post",
            "PreviewHistoricalImport",
            "HistoricalPreviewSummary",
            false,
            false,
        ),
        (
            "/previews/{preview}/summary",
            "get",
            "",
            "HistoricalPreviewSummary",
            false,
            false,
        ),
        (
            "/previews/{preview}/projects/{source}/{page}",
            "get",
            "",
            "binary",
            false,
            false,
        ),
        ("/limit", "get", "", "HistoricalImportLimit", false, false),
        ("/targets", "get", "", "HistoricalImportTarget", true, false),
        (
            "/begin",
            "post",
            "BeginHistoricalImport",
            "HistoricalImportRecord",
            false,
            false,
        ),
        (
            "/imports/{import}",
            "get",
            "",
            "HistoricalImportRecord",
            false,
            false,
        ),
        (
            "/imports/{import}/history",
            "get",
            "",
            "HistoricalHistoryIndex",
            false,
            false,
        ),
        (
            "/imports/{import}/history/index",
            "post",
            "BuildHistoricalHistoryIndex",
            "HistoricalHistoryIndex",
            false,
            false,
        ),
        (
            "/imports/{import}/history/tables",
            "get",
            "",
            "HistoricalHistoryTablesPage",
            false,
            true,
        ),
        (
            "/imports/{import}/history/tables/{table}/rows",
            "get",
            "",
            "HistoricalHistoryRowsPage",
            false,
            true,
        ),
        (
            "/imports/{import}/history/tables/{table}/rows/{position}/cells/{page}",
            "get",
            "",
            "HistoricalHistoryCellsPage",
            false,
            false,
        ),
        (
            "/imports/{import}/history/tables/{table}/rows/{position}/cells/{column}/bytes/{page}",
            "get",
            "",
            "binary",
            false,
            false,
        ),
        (
            "/imports/{import}/chunks/{stream}/{index}",
            "post",
            "binary",
            "HistoricalChunkReceipt",
            false,
            false,
        ),
        (
            "/imports/{import}/chunks/{stream}/{index}",
            "get",
            "",
            "binary",
            false,
            false,
        ),
        (
            "/imports/{import}/verify",
            "post",
            "",
            "HistoricalImportRecord",
            false,
            false,
        ),
        (
            "/imports/{import}/content",
            "get",
            "",
            "HistoricalContentCatalogEntry",
            true,
            true,
        ),
        (
            "/imports/{import}/content/{source}/{page}",
            "get",
            "",
            "binary",
            false,
            false,
        ),
        (
            "/imports/{import}/preview",
            "post",
            "PreviewHistoricalImport",
            "HistoricalImportPreview",
            false,
            false,
        ),
        (
            "/previews/{preview}",
            "get",
            "",
            "HistoricalImportPreview",
            false,
            false,
        ),
        (
            "/previews/{preview}/activate",
            "post",
            "ActivateHistoricalImport",
            "HistoricalActivationReceipt",
            false,
            false,
        ),
        (
            "/previews/{preview}/receipt",
            "get",
            "",
            "HistoricalActivationReceipt",
            false,
            false,
        ),
        (
            "/previews/{preview}/files/{source}/stage",
            "post",
            "binary",
            "HistoricalFileOperation",
            false,
            false,
        ),
        (
            "/previews/{preview}/files/{source}/stage",
            "get",
            "",
            "HistoricalFileOperation",
            false,
            false,
        ),
        (
            "/previews/{preview}/files/{source}/close-unsent",
            "post",
            "CloseHistoricalUnsentFile",
            "HistoricalUnsentFileReceipt",
            false,
            false,
        ),
    ];
    for native in [true, false] {
        let prefix = if native {
            "/api/v2/native/historical-import"
        } else {
            "/api/v2/historical-import/{team}/{project}"
        };
        for (suffix, method, request, response, array, paged) in endpoints {
            let path = format!("{prefix}{suffix}");
            let mut parameters = Vec::new();
            for name in [
                "team", "project", "import", "preview", "stream", "index", "source", "page",
                "table", "position", "column",
            ] {
                if path.contains(&format!("{{{name}}}")) {
                    let schema = match name {
                        "index" | "page" | "table" | "column" => {
                            json!({"type":"integer","minimum":0})
                        }
                        "position" => json!({"type":"integer","minimum":1,"maximum":1000000}),
                        "stream" => json!({"$ref":"#/components/schemas/HistoricalStream"}),
                        "source" if suffix.contains("/files/") => {
                            json!({"type":"string","minLength":1,"maxLength":300})
                        }
                        "source" => json!({"type":"integer","minimum":0}),
                        _ => json!({"type":"string","format":"uuid"}),
                    };
                    parameters
                        .push(json!({"name":name,"in":"path","required":true,"schema":schema}));
                }
            }
            if paged {
                parameters.push(json!({"name":"offset","in":"query","schema":{"type":"integer","minimum":0,"default":0}}));
                let maximum = if suffix.ends_with("/content/catalog") {
                    10
                } else if suffix.contains("/history/tables") {
                    if suffix.ends_with("/rows") { 25 } else { 10 }
                } else {
                    100
                };
                let default = if suffix.ends_with("/content/catalog") {
                    5
                } else if suffix.contains("/history/tables") {
                    10
                } else {
                    50
                };
                parameters.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":maximum,"default":default}}));
            }
            let schema = if response == "binary" {
                json!({"type":"string","format":"binary"})
            } else if array {
                json!({"type":"array","items":{"$ref":format!("#/components/schemas/{response}")}})
            } else {
                json!({"$ref":format!("#/components/schemas/{response}")})
            };
            let media = if response == "binary" {
                "application/octet-stream"
            } else {
                "application/json"
            };
            let mut operation = json!({"servers":[{"url":"/"}],"parameters":parameters,"security":if native {json!([{"daemonBearer":[],"nativeAuthority":[]}])} else {json!([{"androidSession":[]}])},"responses":{"200":{"description":"Current authorized original import result; historical rows never grant execution rights.","content":{media:{"schema":schema}}},"default":{"description":"Missing original bytes, stale authority/quota/target or unconfirmed original IO; no replacement send."}}});
            if !request.is_empty() {
                let media = if request == "binary" {
                    "application/octet-stream"
                } else {
                    "application/json"
                };
                let schema = if request == "binary" {
                    json!({"type":"string","format":"binary"})
                } else {
                    json!({"$ref":format!("#/components/schemas/{request}")})
                };
                operation["requestBody"] =
                    json!({"required":true,"content":{media:{"schema":schema}}});
            }
            document["paths"][path][method] = operation;
        }
    }
    document["paths"]["/api/v2/native/historical-import/limit"]["post"] = json!({"servers":[{"url":"/"}],"security":[{"daemonBearer":[],"nativeAuthority":[]}],"requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/ConfigureHistoricalImportLimit"}}}},"responses":{"200":{"description":"Explicit installation storage policy CAS under current native OS key proof.","content":{"application/json":{"schema":{"$ref":"#/components/schemas/HistoricalImportLimit"}}}},"default":{"description":"Stale policy, retained bytes or native key state; no implicit default limit."}}});
}
