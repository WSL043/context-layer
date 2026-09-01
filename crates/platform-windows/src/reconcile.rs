use std::{io, path::Path};

use context_contracts::FileIdentity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciledFile {
    pub path: std::path::PathBuf,
    pub identity: FileIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileIssueKind {
    AccessDenied,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileIssue {
    pub path: std::path::PathBuf,
    pub kind: ReconcileIssueKind,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub files: Vec<ReconciledFile>,
    pub issues: Vec<ReconcileIssue>,
}

#[cfg(windows)]
pub fn scan_scope(root: impl AsRef<Path>) -> io::Result<ReconcileReport> {
    use std::{fs, os::windows::fs::MetadataExt};

    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    use crate::file_identity;

    let root = root.as_ref();
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("scope root is not a directory: {}", root.display()),
        ));
    }
    let mut report = ReconcileReport::default();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                report.issues.push(issue(directory, error));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.issues.push(issue(directory.clone(), error));
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    report.issues.push(issue(path, error));
                    continue;
                }
            };
            if metadata.is_dir() {
                if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                    pending.push(path);
                }
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            match file_identity(&path) {
                Ok(identity) => report.files.push(ReconciledFile { path, identity }),
                Err(error) => report.issues.push(issue(path, error)),
            }
        }
    }
    report
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    report
        .issues
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(report)
}

#[cfg(windows)]
fn issue(path: std::path::PathBuf, error: io::Error) -> ReconcileIssue {
    ReconcileIssue {
        path,
        kind: if error.kind() == io::ErrorKind::PermissionDenied {
            ReconcileIssueKind::AccessDenied
        } else {
            ReconcileIssueKind::Unavailable
        },
        message: error.to_string(),
    }
}

#[cfg(not(windows))]
pub fn scan_scope(_root: impl AsRef<Path>) -> io::Result<ReconcileReport> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows scope reconciliation is only available on Windows",
    ))
}

#[cfg(all(test, windows))]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use crate::file_identity;

    use super::*;

    #[test]
    fn scan_collects_nested_files_without_following_platform_state() {
        let root = std::env::temp_dir().join(format!("context-layer-{}", Uuid::now_v7()));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let first = root.join("first.txt");
        let second = nested.join("second.txt");
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"two").unwrap();

        let report = scan_scope(&root).unwrap();
        assert!(report.issues.is_empty());
        assert_eq!(report.files.len(), 2);
        assert!(report.files.contains(&ReconciledFile {
            path: second.clone(),
            identity: file_identity(&second).unwrap(),
        }));

        fs::remove_dir_all(root).unwrap();
    }
}
