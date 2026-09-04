use std::io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardSnapshot {
    NonText {
        sequence: u32,
    },
    Text {
        sequence: u32,
        text: String,
        raw_utf16_bytes: u64,
    },
    OversizedText {
        sequence: u32,
        raw_utf16_bytes: u64,
    },
}

impl ClipboardSnapshot {
    pub fn sequence(&self) -> u32 {
        match self {
            Self::NonText { sequence }
            | Self::Text { sequence, .. }
            | Self::OversizedText { sequence, .. } => *sequence,
        }
    }
}

#[cfg(windows)]
pub fn clipboard_snapshot_if_changed(
    last_sequence: Option<u32>,
    max_raw_utf16_bytes: usize,
) -> io::Result<Option<ClipboardSnapshot>> {
    use std::{ptr, slice};

    use windows_sys::Win32::System::{
        DataExchange::{
            CloseClipboard, GetClipboardData, GetClipboardSequenceNumber,
            IsClipboardFormatAvailable, OpenClipboard,
        },
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    };

    if max_raw_utf16_bytes < 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "clipboard raw byte limit must be at least 2 bytes",
        ));
    }

    let sequence = unsafe { GetClipboardSequenceNumber() };
    if sequence == 0 || last_sequence == Some(sequence) {
        return Ok(None);
    }

    let format_available = unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } != 0;
    if !format_available {
        let after = unsafe { GetClipboardSequenceNumber() };
        if after != sequence {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "clipboard changed while checking available formats",
            ));
        }
        return Ok(Some(ClipboardSnapshot::NonText { sequence }));
    }

    if unsafe { OpenClipboard(ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _clipboard = ClipboardGuard;

    let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let raw_size = unsafe { GlobalSize(handle) };
    if raw_size > max_raw_utf16_bytes {
        let after = unsafe { GetClipboardSequenceNumber() };
        if after != sequence {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "clipboard changed while measuring text",
            ));
        }
        return Ok(Some(ClipboardSnapshot::OversizedText {
            sequence,
            raw_utf16_bytes: raw_size as u64,
        }));
    }

    let pointer = unsafe { GlobalLock(handle) } as *const u16;
    if pointer.is_null() {
        return Err(io::Error::last_os_error());
    }
    let _lock = GlobalMemoryLock { handle };

    let unit_count = raw_size / std::mem::size_of::<u16>();
    let units = unsafe { slice::from_raw_parts(pointer, unit_count) };
    let text_end = units.iter().position(|unit| *unit == 0).unwrap_or(unit_count);
    let text = String::from_utf16_lossy(&units[..text_end]);

    let after = unsafe { GetClipboardSequenceNumber() };
    if after != sequence {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "clipboard changed while reading text",
        ));
    }

    Ok(Some(ClipboardSnapshot::Text {
        sequence,
        text,
        raw_utf16_bytes: raw_size as u64,
    }))

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    struct GlobalMemoryLock {
        handle: *mut core::ffi::c_void,
    }
    impl Drop for GlobalMemoryLock {
        fn drop(&mut self) {
            unsafe {
                GlobalUnlock(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn clipboard_snapshot_if_changed(
    _last_sequence: Option<u32>,
    _max_raw_utf16_bytes: usize,
) -> io::Result<Option<ClipboardSnapshot>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "clipboard capture is only available on Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_sequence_is_preserved_for_all_variants() {
        assert_eq!(ClipboardSnapshot::NonText { sequence: 7 }.sequence(), 7);
        assert_eq!(
            ClipboardSnapshot::Text {
                sequence: 8,
                text: "hello".into(),
                raw_utf16_bytes: 12,
            }
            .sequence(),
            8
        );
        assert_eq!(
            ClipboardSnapshot::OversizedText {
                sequence: 9,
                raw_utf16_bytes: 10_000,
            }
            .sequence(),
            9
        );
    }
}
