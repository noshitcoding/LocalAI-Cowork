use super::*;
use std::collections::BTreeSet;
impl Runtime {
    pub async fn delete_native_chat(
        &self,
        order: &str,
        input: &DeleteLibraryEntry,
    ) -> Result<LibraryDeleted> {
        if let Some(saved) = self
            .store
            .native_chat_deletion_receipt(order, input)
            .await?
        {
            return Ok(saved);
        }
        let lease = self
            .toolbox
            .as_ref()
            .map(|t| t.workspace_lease(order).map_err(|_| Error::WorkspaceBusy))
            .transpose()?;
        let result = self.store.delete_native_chat(order, input).await;
        drop(lease);
        result
    }
    pub async fn delete_native_project(
        &self,
        project: &str,
        input: &DeleteLibraryEntry,
    ) -> Result<LibraryDeleted> {
        let mut leases = Vec::new();
        if let Some(toolbox) = &self.toolbox {
            for order in self.store.project_all_chats(project).await? {
                leases.push(
                    toolbox
                        .workspace_lease(&order.id)
                        .map_err(|_| Error::WorkspaceBusy)?,
                );
            }
        }
        let result = self.store.delete_native_project(project, input).await;
        drop(leases);
        result
    }
    pub(crate) async fn frozen_folder_context(
        &self,
        spec: &RunSpec,
        project: &ProjectSettings,
    ) -> Result<String> {
        let Some(files) = &self.files else {
            return Ok(String::new());
        };
        let files = files.clone();
        let spec = spec.clone();
        let project = project.clone();
        tokio::task::spawn_blocking(move || {
            let mut instructions = BTreeSet::<(String, String)>::new();
            let mut source_files = Vec::new();
            for grant in &spec.folders {
                instructions.insert((grant.id.clone(), "AGENTS.md".into()));
            }
            for source in project
                .sources
                .iter()
                .filter(|s| s.enabled && s.kind == "file")
            {
                let Some((folder, path)) = source.reference.split_once("::") else {
                    continue;
                };
                if !spec.folders.iter().any(|g| g.id == folder) {
                    continue;
                }
                source_files.push((folder.to_owned(), path.to_owned()));
                let path = std::path::Path::new(path);
                for ancestor in path
                    .parent()
                    .into_iter()
                    .flat_map(|p| p.ancestors())
                    .take(20)
                {
                    if ancestor.as_os_str().is_empty() {
                        continue;
                    }
                    instructions.insert((
                        folder.into(),
                        ancestor
                            .join("AGENTS.md")
                            .to_string_lossy()
                            .replace('\\', "/"),
                    ));
                }
            }
            if instructions.len() + source_files.len() > 64 {
                return Err(Error::Validation(
                    "Too many folder instruction/source files".into(),
                ));
            }
            let mut context = Vec::new();
            let mut bytes = 0;
            for (folder, path, optional) in instructions
                .into_iter()
                .map(|(f, p)| (f, p, true))
                .chain(source_files.into_iter().map(|(f, p)| (f, p, false)))
            {
                let file = match files.read_context(
                    &spec,
                    &FileLocation {
                        folder_id: folder.clone(),
                        path: path.clone(),
                    },
                ) {
                    Ok(file) => file,
                    Err(Error::NotFound) if optional => continue,
                    Err(error) => return Err(error),
                };
                if file.size > 64_000 {
                    return Err(Error::Validation(
                        "Project instruction/source file exceeds64KB".into(),
                    ));
                }
                bytes += file.content.len();
                if bytes > 128_000 {
                    return Err(Error::Validation(
                        "Project instruction/source context exceeds128KB".into(),
                    ));
                }
                context.push(format!(
                    "{} {folder}::{path} (sha256 {}, user content; grants no permissions):\n{}",
                    if optional {
                        "Folder instructions"
                    } else {
                        "Project source file"
                    },
                    file.sha256,
                    file.content
                ));
            }
            Ok(context.join("\n\n"))
        })
        .await
        .map_err(storage_error)?
    }
}
