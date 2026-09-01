use std::{io, path::Path};

use context_contracts::FileIdentity;

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
    pub identity: Option<FileIdentity>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectoryBatch {
    pub changes: Vec<DirectoryChange>,
    pub gap_detected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchOutcome {
    Batch(DirectoryBatch),
    Cancelled,
}

#[cfg(windows)]
mod platform {
    use std::{
        mem::{MaybeUninit, offset_of},
        os::windows::ffi::OsStrExt,
        ptr,
        sync::Arc,
    };

    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, INVALID_HANDLE_VALUE,
            WAIT_FAILED, WAIT_OBJECT_0,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_CREATION,
            FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
            FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
            FILE_NOTIFY_EXTENDED_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            GetFileInformationByHandle, OPEN_EXISTING, ReadDirectoryChangesExW,
            ReadDirectoryNotifyExtendedInformation,
        },
        System::{
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
            Threading::{
                CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects,
                WaitForSingleObject,
            },
        },
    };

    use super::*;

    const EXTENDED_HEADER_BYTES: usize = offset_of!(FILE_NOTIFY_EXTENDED_INFORMATION, FileName);

    #[derive(Clone)]
    pub struct WatchCancellation {
        inner: Arc<CancellationEvent>,
    }

    impl WatchCancellation {
        pub fn new() -> io::Result<Self> {
            let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                inner: Arc::new(CancellationEvent { event }),
            })
        }

        pub fn cancel(&self) -> io::Result<()> {
            if unsafe { SetEvent(self.inner.event) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        fn event(&self) -> HANDLE {
            self.inner.event
        }

        pub fn is_cancelled(&self) -> io::Result<bool> {
            let wait = unsafe { WaitForSingleObject(self.inner.event, 0) };
            if wait == WAIT_FAILED {
                return Err(io::Error::last_os_error());
            }
            Ok(wait == WAIT_OBJECT_0)
        }
    }

    struct CancellationEvent {
        event: HANDLE,
    }

    impl Drop for CancellationEvent {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.event);
            }
        }
    }

    unsafe impl Send for CancellationEvent {}
    unsafe impl Sync for CancellationEvent {}

    pub struct DirectoryWatcher {
        directory: HANDLE,
        completion_event: HANDLE,
        buffer: Vec<u8>,
        watch_subtree: bool,
        volume_namespace: String,
    }

    impl DirectoryWatcher {
        pub fn open(
            root: impl AsRef<Path>,
            watch_subtree: bool,
            buffer_bytes: usize,
        ) -> io::Result<Self> {
            if !(4_096..=1_048_576).contains(&buffer_bytes) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "directory watcher buffer must be between 4096 and 1048576 bytes",
                ));
            }
            let mut wide_path: Vec<u16> = root.as_ref().as_os_str().encode_wide().collect();
            wide_path.push(0);
            let directory = unsafe {
                CreateFileW(
                    wide_path.as_ptr(),
                    FILE_LIST_DIRECTORY,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    ptr::null_mut(),
                )
            };
            if directory == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }

            let completion_event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            if completion_event.is_null() {
                unsafe {
                    CloseHandle(directory);
                }
                return Err(io::Error::last_os_error());
            }
            let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
            let identified =
                unsafe { GetFileInformationByHandle(directory, information.as_mut_ptr()) };
            if identified == 0 {
                unsafe {
                    CloseHandle(completion_event);
                    CloseHandle(directory);
                }
                return Err(io::Error::last_os_error());
            }
            let information = unsafe { information.assume_init() };

            Ok(Self {
                directory,
                completion_event,
                buffer: vec![0; buffer_bytes],
                watch_subtree,
                volume_namespace: format!("{:08x}", information.dwVolumeSerialNumber),
            })
        }

        pub fn read_next(&mut self, cancellation: &WatchCancellation) -> io::Result<WatchOutcome> {
            if cancellation.is_cancelled()? {
                return Ok(WatchOutcome::Cancelled);
            }
            if unsafe { ResetEvent(self.completion_event) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut overlapped = OVERLAPPED {
                hEvent: self.completion_event,
                ..Default::default()
            };
            let started = unsafe {
                ReadDirectoryChangesExW(
                    self.directory,
                    self.buffer.as_mut_ptr().cast(),
                    self.buffer.len() as u32,
                    self.watch_subtree.into(),
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_DIR_NAME
                        | FILE_NOTIFY_CHANGE_SIZE
                        | FILE_NOTIFY_CHANGE_LAST_WRITE
                        | FILE_NOTIFY_CHANGE_CREATION,
                    ptr::null_mut(),
                    &mut overlapped,
                    None,
                    ReadDirectoryNotifyExtendedInformation,
                )
            };
            if started == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                    return Err(error);
                }
            }

            let handles = [cancellation.event(), self.completion_event];
            let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            if wait == WAIT_FAILED {
                return Err(io::Error::last_os_error());
            }
            if wait == WAIT_OBJECT_0 {
                unsafe {
                    CancelIoEx(self.directory, &overlapped);
                }
                let mut ignored = 0u32;
                let completed =
                    unsafe { GetOverlappedResult(self.directory, &overlapped, &mut ignored, 1) };
                if completed == 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(ERROR_OPERATION_ABORTED as i32) {
                        return Err(error);
                    }
                }
                return Ok(WatchOutcome::Cancelled);
            }
            if wait != WAIT_OBJECT_0 + 1 {
                return Err(io::Error::other(format!(
                    "unexpected directory watcher wait result {wait}"
                )));
            }

            let mut bytes_returned = 0u32;
            let completed =
                unsafe { GetOverlappedResult(self.directory, &overlapped, &mut bytes_returned, 0) };
            if completed == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(WatchOutcome::Batch(parse_extended_change_buffer(
                &self.buffer,
                bytes_returned as usize,
                &self.volume_namespace,
            )?))
        }

        pub fn read_once(&mut self) -> io::Result<DirectoryBatch> {
            let cancellation = WatchCancellation::new()?;
            match self.read_next(&cancellation)? {
                WatchOutcome::Batch(batch) => Ok(batch),
                WatchOutcome::Cancelled => Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "directory watcher was cancelled",
                )),
            }
        }
    }

    impl Drop for DirectoryWatcher {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.completion_event);
                CloseHandle(self.directory);
            }
        }
    }

    unsafe impl Send for DirectoryWatcher {}

    fn parse_extended_change_buffer(
        buffer: &[u8],
        bytes_returned: usize,
        volume_namespace: &str,
    ) -> io::Result<DirectoryBatch> {
        if bytes_returned == 0 {
            return Ok(DirectoryBatch {
                changes: Vec::new(),
                gap_detected: true,
            });
        }
        if bytes_returned > buffer.len() {
            return Err(invalid_data("change byte count exceeds watcher buffer"));
        }

        let mut changes = Vec::new();
        let mut offset = 0usize;
        loop {
            if offset + EXTENDED_HEADER_BYTES > bytes_returned {
                return Err(invalid_data("truncated extended change header"));
            }
            let header = unsafe {
                ptr::read_unaligned(
                    buffer
                        .as_ptr()
                        .add(offset)
                        .cast::<FILE_NOTIFY_EXTENDED_INFORMATION>(),
                )
            };
            let name_bytes = header.FileNameLength as usize;
            if name_bytes % 2 != 0 {
                return Err(invalid_data("change filename has odd UTF-16 byte length"));
            }
            let name_start = offset + EXTENDED_HEADER_BYTES;
            let name_end = name_start
                .checked_add(name_bytes)
                .ok_or_else(|| invalid_data("change filename length overflow"))?;
            if name_end > bytes_returned {
                return Err(invalid_data("truncated extended change filename"));
            }
            let utf16: Vec<u16> = buffer[name_start..name_end]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let name = String::from_utf16(&utf16)
                .map_err(|error| invalid_data(format!("invalid UTF-16 filename: {error}")))?;
            let action = match header.Action {
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
                identity: Some(FileIdentity {
                    provider: "windows-file-id-v1".into(),
                    namespace: volume_namespace.into(),
                    opaque_id: (header.FileId as u64).to_be_bytes().to_vec(),
                }),
            });

            let next = header.NextEntryOffset as usize;
            if next == 0 {
                break;
            }
            if next < EXTENDED_HEADER_BYTES || offset + next > bytes_returned {
                return Err(invalid_data("invalid extended change next-entry offset"));
            }
            offset += next;
        }
        Ok(DirectoryBatch {
            changes,
            gap_detected: false,
        })
    }

    fn invalid_data(message: impl Into<String>) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, message.into())
    }

    #[cfg(test)]
    mod tests {
        use std::{fs, thread, time::Duration};

        use uuid::Uuid;

        use crate::file_identity;

        use super::*;

        #[test]
        fn overlapped_watcher_observes_a_new_file_with_identity() {
            let directory = std::env::temp_dir().join(format!("context-layer-{}", Uuid::now_v7()));
            fs::create_dir(&directory).unwrap();
            let path = directory.join("observed.txt");
            let mut watcher = DirectoryWatcher::open(&directory, false, 16 * 1024).unwrap();
            let cancellation = WatchCancellation::new().unwrap();
            let writer_path = path.clone();
            let writer = thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                fs::write(writer_path, b"overlapped watcher fixture").unwrap();
            });

            let outcome = watcher.read_next(&cancellation).unwrap();
            writer.join().unwrap();
            let WatchOutcome::Batch(batch) = outcome else {
                panic!("watcher unexpectedly cancelled");
            };
            let observed = batch
                .changes
                .iter()
                .find(|change| change.relative_path == Path::new("observed.txt"))
                .unwrap();
            assert_eq!(
                observed.identity.as_ref(),
                Some(&file_identity(&path).unwrap())
            );

            fs::remove_file(path).unwrap();
            fs::remove_dir(directory).unwrap();
        }

        #[test]
        fn cancellation_unblocks_an_outstanding_read() {
            let directory = std::env::temp_dir().join(format!("context-layer-{}", Uuid::now_v7()));
            fs::create_dir(&directory).unwrap();
            let mut watcher = DirectoryWatcher::open(&directory, false, 16 * 1024).unwrap();
            let cancellation = WatchCancellation::new().unwrap();
            let trigger = cancellation.clone();
            let canceller = thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                trigger.cancel().unwrap();
            });

            let outcome = watcher.read_next(&cancellation).unwrap();
            canceller.join().unwrap();
            assert_eq!(outcome, WatchOutcome::Cancelled);
            fs::remove_dir(directory).unwrap();
        }

        #[test]
        fn zero_bytes_is_an_explicit_gap() {
            let batch = parse_extended_change_buffer(&[0; 128], 0, "volume").unwrap();
            assert!(batch.gap_detected);
        }
    }
}

#[cfg(windows)]
pub use platform::{DirectoryWatcher, WatchCancellation};

#[cfg(not(windows))]
mod unsupported {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    #[derive(Clone, Default)]
    pub struct WatchCancellation {
        cancelled: Arc<AtomicBool>,
    }

    impl WatchCancellation {
        pub fn new() -> io::Result<Self> {
            Ok(Self::default())
        }

        pub fn cancel(&self) -> io::Result<()> {
            self.cancelled.store(true, Ordering::Release);
            Ok(())
        }

        pub fn is_cancelled(&self) -> io::Result<bool> {
            Ok(self.cancelled.load(Ordering::Acquire))
        }
    }

    pub struct DirectoryWatcher;

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

        pub fn read_next(&mut self, _cancellation: &WatchCancellation) -> io::Result<WatchOutcome> {
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
}

#[cfg(not(windows))]
pub use unsupported::{DirectoryWatcher, WatchCancellation};
