//! Built-in tool classes shared by preflight, policy and workflow scope checks.

/// Reading document/image tool. It never changes a granted folder.
pub const DOCUMENT_READ_TOOL: &str = "document_read";
/// Document/image tools that publish one new file or directory, never an overwrite.
pub const DOCUMENT_WRITE_TOOLS: [&str; 2] = ["document_write", "image_edit"];

/// Tools retained by the read-only access mode. Every read-only reduction uses this list.
pub fn read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "list_files"
            | "read_text_file"
            | "stat_file"
            | "list_artifacts"
            | "read_artifact"
            | "request_input"
            | DOCUMENT_READ_TOOL
    )
}

pub fn document_tool(name: &str) -> bool {
    name == DOCUMENT_READ_TOOL || DOCUMENT_WRITE_TOOLS.contains(&name)
}

pub fn document_write_tool(name: &str) -> bool {
    DOCUMENT_WRITE_TOOLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_keeps_reads_and_document_reads_only() {
        for tool in [
            "list_files",
            "read_text_file",
            "stat_file",
            "list_artifacts",
            "read_artifact",
            "request_input",
            "document_read",
        ] {
            assert!(read_only_tool(tool), "{tool}");
        }
        for tool in [
            "write_text_file",
            "document_write",
            "image_edit",
            "office",
            "execute_program",
            "mcp_x",
        ] {
            assert!(!read_only_tool(tool), "{tool}");
        }
        assert!(
            document_tool("image_edit")
                && document_write_tool("document_write")
                && !document_write_tool("document_read")
        );
    }
}
