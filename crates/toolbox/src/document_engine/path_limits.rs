use std::path::{Path, PathBuf};
use uuid::Uuid;

fn stage(root: &Path) -> PathBuf {
    root.join("document-stages").join(Uuid::nil().to_string())
}

/// The generated UUID has the same fixed length as every real document stage.
pub(super) fn document_working_directory_fits(root: &Path) -> bool {
    cowork_executor::working_directory_path_fits(&stage(root))
}

/// LaTeX also launches inside its source directory, and has a longer probe name.
pub(super) fn latex_working_directories_fit(root: &Path) -> bool {
    [
        stage(root).join("in/tex"),
        stage(root).join("work/latex"),
        root.join("document-stages")
            .join(format!("tex-probe-{}", Uuid::nil().simple())),
    ]
    .iter()
    .all(|path| cowork_executor::working_directory_path_fits(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn document_and_nested_latex_paths_have_separate_utf16_budgets() {
        use std::os::windows::ffi::OsStrExt;
        let root = PathBuf::from(format!("C:\\{}", "x".repeat(202)));
        assert_eq!(stage(&root).as_os_str().encode_wide().count(), 258);
        assert!(document_working_directory_fits(&root));
        assert!(!document_working_directory_fits(&root.join("x")));
        assert!(!latex_working_directories_fit(&root));
        let shorter = PathBuf::from(format!("C:\\{}", "x".repeat(191)));
        assert!(latex_working_directories_fit(&shorter));
        assert!(!latex_working_directories_fit(&shorter.join("x")));
    }

    #[cfg(not(windows))]
    #[test]
    fn windows_cwd_budget_does_not_restrict_linux_document_paths() {
        let root = PathBuf::from(format!("/private/{}", "long/".repeat(100)));
        assert!(document_working_directory_fits(&root));
        assert!(latex_working_directories_fit(&root));
    }
}
