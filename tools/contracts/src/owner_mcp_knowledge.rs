use serde_json::{Value, json};
pub fn extend(document: &mut Value) {
    for (path, method, input, output, array, native) in [
        (
            "/projects/{project}/skills",
            "get",
            "",
            "Skill",
            true,
            false,
        ),
        (
            "/projects/{project}/skills",
            "post",
            "WriteSkill",
            "Skill",
            false,
            false,
        ),
        (
            "/projects/{project}/knowledge/import",
            "post",
            "KnowledgeImportRequest",
            "KnowledgeImportResult",
            false,
            false,
        ),
        (
            "/projects/{project}/knowledge-folders",
            "get",
            "",
            "KnowledgeFolders",
            false,
            false,
        ),
        (
            "/projects/{project}/knowledge-folders",
            "put",
            "WriteKnowledgeFolders",
            "KnowledgeFolders",
            false,
            false,
        ),
        (
            "/projects/{project}/knowledge-graphs",
            "get",
            "",
            "KnowledgeTopicGraph",
            true,
            false,
        ),
        (
            "/projects/{project}/knowledge-graphs",
            "post",
            "BuildKnowledgeGraph",
            "KnowledgeTopicGraph",
            false,
            false,
        ),
        (
            "/projects/{project}/knowledge-graphs",
            "delete",
            "BuildKnowledgeGraph",
            "",
            false,
            false,
        ),
        (
            "/projects/{project}/knowledge-graphs/search",
            "post",
            "SearchKnowledgeGraph",
            "KnowledgeGraphResults",
            false,
            false,
        ),
        (
            "/projects/{project}/schedules",
            "get",
            "",
            "ProductSchedule",
            true,
            false,
        ),
        (
            "/projects/{project}/schedules",
            "post",
            "WriteSchedule",
            "ProductSchedule",
            false,
            false,
        ),
        (
            "/mcp/capabilities",
            "get",
            "",
            "OwnerMcpCapabilities",
            false,
            false,
        ),
        (
            "/mcp/credential",
            "post",
            "ConfigureOwnerMcpCredential",
            "OwnerMcpCredential",
            false,
            false,
        ),
        (
            "/mcp/credential/{project}/{server}",
            "get",
            "",
            "OwnerMcpCredential",
            false,
            false,
        ),
        (
            "/mcp/discovery",
            "post",
            "OwnerMcpDiscovery",
            "OwnerMcpCatalog",
            false,
            false,
        ),
        (
            "/mcp/stdio",
            "post",
            "ConfigureOwnerMcpStdio",
            "OwnerMcpCatalog",
            false,
            false,
        ),
        (
            "/mcp/stdio/{project}",
            "get",
            "",
            "NativeMcpFactory",
            true,
            false,
        ),
        (
            "/mcp/stdio/{project}/{server}/disconnect",
            "post",
            "OwnerMcpExpectedRevision",
            "OwnerMcpRevision",
            false,
            false,
        ),
        (
            "/native/mcp/credential",
            "post",
            "ConfigureOwnerMcpCredential",
            "OwnerMcpCredential",
            false,
            true,
        ),
        (
            "/native/mcp/credential/{project}/{server}",
            "get",
            "",
            "OwnerMcpCredential",
            false,
            true,
        ),
        (
            "/native/mcp/stdio",
            "post",
            "ConfigureOwnerMcpStdio",
            "OwnerMcpCatalog",
            false,
            true,
        ),
        (
            "/native/mcp/stdio/{project}",
            "get",
            "",
            "NativeMcpFactory",
            true,
            true,
        ),
        (
            "/native/mcp/stdio/{project}/{server}/disconnect",
            "post",
            "OwnerMcpExpectedRevision",
            "OwnerMcpRevision",
            false,
            true,
        ),
    ] {
        let schema = if output.is_empty() {
            json!({"type":"object"})
        } else if array {
            json!({"type":"array","items":{"$ref":format!("#/components/schemas/{output}")}})
        } else {
            json!({"$ref":format!("#/components/schemas/{output}")})
        };
        let mut operation = json!({"security":[{"daemonBearer":[]}],"responses":{"200":{"description":"Saved scoped configuration, discovery, or original import acknowledgement","content":{"application/json":{"schema":schema}}},"400":{"description":"Invalid configuration or import"},"401":{"description":"Owner authentication required"},"403":{"description":"Forbidden origin, custody boundary, or native confirmation missing"},"404":{"description":"No resource in this project"},"409":{"description":"Revision or idempotency conflict"}}});
        if native {
            operation["security"] = json!([{"daemonBearer":[],"nativeAuthority":[]}]);
            operation["description"] = json!(
                "Requires owner bearer and separate native confirmation credential; browser owner bearer alone is insufficient."
            );
        } else if path.starts_with("/mcp") {
            operation["description"] = json!(
                "Single-owner server configuration only. Never an actor-session or native-confirmation fallback."
            );
        }
        if !input.is_empty() {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        let parameters:Vec<_>=["project","server"].iter().filter(|name|path.contains(&format!("{{{name}}}"))).map(|name|json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}})).collect();
        if !parameters.is_empty() {
            operation["parameters"] = json!(parameters);
        }
        if path.contains("knowledge-folders") || path.contains("knowledge-graphs") {
            operation["parameters"].as_array_mut().unwrap().push(
                json!({"name":"personal","in":"query","schema":{"type":"boolean","default":false}}),
            );
        }
        document["paths"][path][method] = operation;
    }
    document["paths"]["/orders/{order}/toolbox/files"]["get"] = json!({
        "security":[{"daemonBearer":[]}],"description":"Owner-scoped managed tool output. Bytes, filename, size and SHA256 refer to the same read. An expected digest rejects changed content.",
        "parameters":[{"name":"order","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},{"name":"path","in":"query","required":true,"schema":{"type":"string"}},{"name":"expected_sha256","in":"query","required":false,"schema":{"type":"string","pattern":"^[0-9a-f]{64}$"}}],
        "responses":{"200":{"description":"Verified-download metadata and exact binary bytes","headers":{"x-cowork-file-name":{"description":"Percent-encoded UTF-8 basename","schema":{"type":"string"}},"x-cowork-sha256":{"schema":{"type":"string","pattern":"^[0-9a-f]{64}$"}},"Content-Length":{"schema":{"type":"integer","minimum":0,"maximum":33554432}}},"content":{"application/octet-stream":{"schema":{"type":"string","format":"binary"}}}},"400":{"description":"Invalid managed path or file size"},"401":{"description":"Owner authentication required"},"404":{"description":"No file in this order"},"409":{"description":"File changed or workspace busy"}}
    });
}
