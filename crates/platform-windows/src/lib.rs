use std::{io, path::Path};

use context_contracts::FileIdentity;

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
}
