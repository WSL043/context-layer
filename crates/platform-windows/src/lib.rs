use std::{io, path::Path};

use context_contracts::FileIdentity;

mod reconcile;
mod watcher;

pub use reconcile::{
    ReconcileIssue, ReconcileIssueKind, ReconcileReport, ReconciledFile, scan_scope,
};
pub use watcher::{
    DirectoryAction, DirectoryBatch, DirectoryChange, DirectoryWatcher, WatchCancellation,
    WatchOutcome,
};

/// Converts Win32 verbatim paths to the stable absolute form used in event contracts.
pub fn contract_path(path: impl AsRef<Path>) -> String {
    let value = path.as_ref().to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        rest.into()
    } else {
        value.into_owned()
    }
}

#[cfg(windows)]
pub fn file_identity(path: impl AsRef<Path>) -> io::Result<FileIdentity> {
    use std::{fs::File, mem::MaybeUninit, os::windows::io::AsRawHandle};

    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = File::open(path)?;
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    let information = unsafe { information.assume_init() };
    let file_index = ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64;

    Ok(FileIdentity {
        provider: "windows-file-id-v1".into(),
        namespace: format!("{:08x}", information.dwVolumeSerialNumber),
        opaque_id: file_index.to_be_bytes().to_vec(),
    })
}

#[cfg(not(windows))]
pub fn file_identity(_path: impl AsRef<Path>) -> io::Result<FileIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows file identity is only available on Windows",
    ))
}

#[cfg(all(test, windows))]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::*;

    #[test]
    fn identity_survives_rename() {
        let directory = std::env::temp_dir().join(format!("context-layer-{}", Uuid::now_v7()));
        fs::create_dir(&directory).unwrap();
        let before = directory.join("before.txt");
        let after = directory.join("after.txt");
        fs::write(&before, b"identity fixture").unwrap();

        let first = file_identity(&before).unwrap();
        fs::rename(&before, &after).unwrap();
        let second = file_identity(&after).unwrap();

        assert_eq!(first, second);
        fs::remove_file(&after).unwrap();
        fs::remove_dir(&directory).unwrap();
    }

    #[test]
    fn verbatim_paths_are_normalized_for_cross_adapter_matching() {
        assert_eq!(
            contract_path(r"\\?\C:\Users\Example\Downloads\report.pdf"),
            r"C:\Users\Example\Downloads\report.pdf"
        );
        assert_eq!(
            contract_path(r"\\?\UNC\server\share\report.pdf"),
            r"\\server\share\report.pdf"
        );
    }
}
