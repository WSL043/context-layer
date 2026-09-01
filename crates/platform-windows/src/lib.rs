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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryAction {
    Added,
    Removed,
    Modified,
    RenamedFrom,
    RenamedTo,
    Unknown(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryChange {
    pub action: DirectoryAction,
    pub relative_path: std::path::PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectoryBatch {
    pub changes: Vec<DirectoryChange>,
    pub gap_detected: bool,
}

#[cfg(any(windows, test))]
const CHANGE_HEADER_BYTES: usize = 12;

#[cfg(any(windows, test))]
fn parse_change_buffer(buffer: &[u8], bytes_returned: usize) -> io::Result<DirectoryBatch> {
    if bytes_returned == 0 {
        return Ok(DirectoryBatch {
            changes: Vec::new(),
            gap_detected: true,
        });
    }
    if bytes_returned > buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "directory change byte count exceeds the supplied buffer",
        ));
    }

    let mut changes = Vec::new();
    let mut offset = 0usize;
    loop {
        if offset + CHANGE_HEADER_BYTES > bytes_returned {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated directory change header",
            ));
        }
        let read_u32 = |start: usize| {
            u32::from_le_bytes(
                buffer[start..start + 4]
                    .try_into()
                    .expect("four-byte slice"),
            )
        };
        let next_offset = read_u32(offset) as usize;
        let raw_action = read_u32(offset + 4);
        let name_bytes = read_u32(offset + 8) as usize;
        if name_bytes % 2 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "directory change filename is not valid UTF-16 bytes",
            ));
        }
        let name_start = offset + CHANGE_HEADER_BYTES;
        let name_end = name_start.checked_add(name_bytes).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "directory change length overflow",
            )
        })?;
        if name_end > bytes_returned {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated directory change filename",
            ));
        }

        let utf16: Vec<u16> = buffer[name_start..name_end]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let name = String::from_utf16(&utf16).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid UTF-16 directory change filename: {error}"),
            )
        })?;
        let action = match raw_action {
            1 => DirectoryAction::Added,
            2 => DirectoryAction::Removed,
            3 => DirectoryAction::Modified,
            4 => DirectoryAction::RenamedFrom,
            5 => DirectoryAction::RenamedTo,
            other => DirectoryAction::Unknown(other),
        };
        changes.push(DirectoryChange {
            action,
            relative_path: name.into(),
        });

        if next_offset == 0 {
            break;
        }
        if next_offset < CHANGE_HEADER_BYTES || offset + next_offset > bytes_returned {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid directory change next-entry offset",
            ));
        }
        offset += next_offset;
    }

    Ok(DirectoryBatch {
        changes,
        gap_detected: false,
    })
}

#[cfg(windows)]
pub struct DirectoryWatcher {
    handle: windows_sys::Win32::Foundation::HANDLE,
    buffer: Vec<u8>,
    watch_subtree: bool,
}

#[cfg(windows)]
impl DirectoryWatcher {
    pub fn open(
        root: impl AsRef<Path>,
        watch_subtree: bool,
        buffer_bytes: usize,
    ) -> io::Result<Self> {
        use std::os::windows::ffi::OsStrExt;

        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            Storage::FileSystem::{
                CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE,
                FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            },
        };

        if !(4_096..=1_048_576).contains(&buffer_bytes) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory watcher buffer must be between 4096 and 1048576 bytes",
            ));
        }
        let mut wide_path: Vec<u16> = root.as_ref().as_os_str().encode_wide().collect();
        wide_path.push(0);
        let handle = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            handle,
            buffer: vec![0; buffer_bytes],
            watch_subtree,
        })
    }

    pub fn read_once(&mut self) -> io::Result<DirectoryBatch> {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
            FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, ReadDirectoryChangesW,
        };

        let mut bytes_returned = 0u32;
        let succeeded = unsafe {
            ReadDirectoryChangesW(
                self.handle,
                self.buffer.as_mut_ptr().cast(),
                self.buffer.len() as u32,
                self.watch_subtree.into(),
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_CREATION,
                &mut bytes_returned,
                std::ptr::null_mut(),
                None,
            )
        };
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        parse_change_buffer(&self.buffer, bytes_returned as usize)
    }
}

#[cfg(windows)]
impl Drop for DirectoryWatcher {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(not(windows))]
pub struct DirectoryWatcher;

#[cfg(not(windows))]
impl DirectoryWatcher {
    pub fn open(
        _root: impl AsRef<Path>,
        _watch_subtree: bool,
        _buffer_bytes: usize,
    ) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows directory watching is only available on Windows",
        ))
    }

    pub fn read_once(&mut self) -> io::Result<DirectoryBatch> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows directory watching is only available on Windows",
        ))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::{fs, thread, time::Duration};

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
    fn directory_watcher_observes_a_new_file() {
        let directory = std::env::temp_dir().join(format!("context-layer-{}", Uuid::now_v7()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("observed.txt");
        let mut watcher = DirectoryWatcher::open(&directory, false, 16 * 1024).unwrap();
        let writer_path = path.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            fs::write(writer_path, b"directory watcher fixture").unwrap();
        });

        let batch = watcher.read_once().unwrap();
        writer.join().unwrap();

        assert!(!batch.gap_detected);
        assert!(batch.changes.iter().any(|change| {
            change.relative_path == Path::new("observed.txt")
                && matches!(
                    change.action,
                    DirectoryAction::Added | DirectoryAction::Modified
                )
        }));
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    #[test]
    fn zero_bytes_is_an_explicit_collector_gap() {
        let batch = parse_change_buffer(&[0; 16], 0).unwrap();
        assert!(batch.gap_detected);
        assert!(batch.changes.is_empty());
    }

    #[test]
    fn malformed_change_data_is_rejected() {
        let error = parse_change_buffer(&[0; 8], 8).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
