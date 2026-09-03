use std::io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForegroundActivity {
    pub process_id: u32,
    pub process_path: Option<String>,
    pub window_title: String,
}

#[cfg(windows)]
pub fn foreground_activity() -> io::Result<Option<ForegroundActivity>> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        },
    };

    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return Ok(None);
    }

    let mut process_id = 0u32;
    let thread_id = unsafe { GetWindowThreadProcessId(window, &mut process_id) };
    if thread_id == 0 || process_id == 0 {
        return Err(io::Error::last_os_error());
    }

    let title_length = unsafe { GetWindowTextLengthW(window) };
    let window_title = if title_length <= 0 {
        String::new()
    } else {
        let mut buffer = vec![0u16; title_length as usize + 1];
        let copied = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
        if copied <= 0 {
            String::new()
        } else {
            String::from_utf16_lossy(&buffer[..copied as usize])
        }
    };

    let process_path = unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if handle.is_null() {
            None
        } else {
            let mut buffer = vec![0u16; 32_768];
            let mut length = buffer.len() as u32;
            let succeeded = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length);
            let _ = CloseHandle(handle);
            if succeeded == 0 {
                None
            } else {
                Some(String::from_utf16_lossy(&buffer[..length as usize]))
            }
        }
    };

    Ok(Some(ForegroundActivity {
        process_id,
        process_path,
        window_title,
    }))
}

#[cfg(not(windows))]
pub fn foreground_activity() -> io::Result<Option<ForegroundActivity>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "foreground activity is only available on Windows",
    ))
}

#[cfg(windows)]
pub fn input_idle_millis() -> io::Result<u64> {
    use std::mem::size_of;

    use windows_sys::Win32::{
        System::SystemInformation::GetTickCount,
        UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
    };

    let mut last_input = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    let succeeded = unsafe { GetLastInputInfo(&mut last_input) };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }

    // LASTINPUTINFO uses a 32-bit tick count. Wrapping subtraction keeps the
    // result correct across the roughly 49.7-day GetTickCount rollover.
    let now = unsafe { GetTickCount() };
    Ok(now.wrapping_sub(last_input.dwTime) as u64)
}

#[cfg(not(windows))]
pub fn input_idle_millis() -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "input idle time is only available on Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_activity_is_comparable_for_change_detection() {
        let first = ForegroundActivity {
            process_id: 42,
            process_path: Some(r"C:\Program Files\Example\example.exe".into()),
            window_title: "Document A".into(),
        };
        let replay = first.clone();
        let mut changed = first.clone();
        changed.window_title = "Document B".into();

        assert_eq!(first, replay);
        assert_ne!(first, changed);
    }
}
