use std::{
    io::{self, Read, Write},
    mem::size_of,
    os::windows::ffi::OsStrExt,
    ptr,
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE,
        HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    },
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
    },
    System::{
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
        },
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;

pub fn current_user_pipe_name() -> io::Result<String> {
    Ok(format!(
        r"\\.\pipe\context-layer-v1-{}",
        current_user_sid()?
    ))
}

pub struct NamedPipeServer {
    handle: HANDLE,
}

impl NamedPipeServer {
    pub fn bind_current_user() -> io::Result<Self> {
        let pipe_name = current_user_pipe_name()?;
        let wide_name = wide_null(&pipe_name);
        let descriptor = SecurityDescriptor::for_current_user()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.pointer.cast(),
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateNamedPipeW(
                wide_name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                0,
                &attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle })
    }

    pub fn accept(mut self) -> io::Result<NamedPipeConnection> {
        let connected = unsafe { ConnectNamedPipe(self.handle, ptr::null_mut()) };
        if connected == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                return Err(error);
            }
        }
        let handle = self.handle;
        self.handle = INVALID_HANDLE_VALUE;
        Ok(NamedPipeConnection {
            handle,
            server_end: true,
        })
    }
}

impl Drop for NamedPipeServer {
    fn drop(&mut self) {
        close_if_valid(self.handle);
    }
}

pub struct NamedPipeClient;

impl NamedPipeClient {
    pub fn connect_current_user(timeout_ms: u32) -> io::Result<NamedPipeConnection> {
        let pipe_name = current_user_pipe_name()?;
        let wide_name = wide_null(&pipe_name);
        let available = unsafe { WaitNamedPipeW(wide_name.as_ptr(), timeout_ms) };
        if available == 0 {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe {
            CreateFileW(
                wide_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(NamedPipeConnection {
            handle,
            server_end: false,
        })
    }
}

pub struct NamedPipeConnection {
    handle: HANDLE,
    server_end: bool,
}

impl Read for NamedPipeConnection {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let length = buffer.len().min(u32::MAX as usize) as u32;
        let mut bytes_read = 0u32;
        let succeeded = unsafe {
            ReadFile(
                self.handle,
                buffer.as_mut_ptr(),
                length,
                &mut bytes_read,
                ptr::null_mut(),
            )
        };
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(bytes_read as usize)
    }
}

impl Write for NamedPipeConnection {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let length = buffer.len().min(u32::MAX as usize) as u32;
        let mut bytes_written = 0u32;
        let succeeded = unsafe {
            WriteFile(
                self.handle,
                buffer.as_ptr(),
                length,
                &mut bytes_written,
                ptr::null_mut(),
            )
        };
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(bytes_written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for NamedPipeConnection {
    fn drop(&mut self) {
        if self.server_end {
            unsafe {
                DisconnectNamedPipe(self.handle);
            }
        }
        close_if_valid(self.handle);
    }
}

struct SecurityDescriptor {
    pointer: PSECURITY_DESCRIPTOR,
}

impl SecurityDescriptor {
    fn for_current_user() -> io::Result<Self> {
        let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", current_user_sid()?);
        let wide_sddl = wide_null(&sddl);
        let mut pointer = ptr::null_mut();
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide_sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut pointer,
                ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { pointer })
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.pointer.cast());
        }
    }
}

fn current_user_sid() -> io::Result<String> {
    let mut token = ptr::null_mut();
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 {
        return Err(io::Error::last_os_error());
    }

    let result = (|| {
        let mut required = 0u32;
        let first =
            unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required) };
        if first != 0
            || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0u8; required as usize];
        let fetched = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        };
        if fetched == 0 {
            return Err(io::Error::last_os_error());
        }
        let token_user = unsafe { ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_USER>()) };
        let mut sid_text = ptr::null_mut();
        let converted = unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_text) };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        let length = unsafe {
            let mut length = 0usize;
            while *sid_text.add(length) != 0 {
                length += 1;
            }
            length
        };
        let value =
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sid_text, length) });
        unsafe {
            LocalFree(sid_text.cast());
        }
        Ok(value)
    })();

    unsafe {
        CloseHandle(token);
    }
    result
}

fn wide_null(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn close_if_valid(handle: HANDLE) {
    if handle != INVALID_HANDLE_VALUE && !handle.is_null() {
        unsafe {
            CloseHandle(handle);
        }
    }
}

unsafe impl Send for NamedPipeServer {}
unsafe impl Send for NamedPipeConnection {}

#[cfg(test)]
mod tests {
    use std::thread;

    use serde::{Deserialize, Serialize};

    use crate::{read_frame, write_frame};

    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Message {
        text: String,
    }

    #[test]
    fn current_user_only_pipe_round_trips_a_frame() {
        let server = NamedPipeServer::bind_current_user().unwrap();
        let client = thread::spawn(|| {
            let mut connection = NamedPipeClient::connect_current_user(5_000).unwrap();
            write_frame(
                &mut connection,
                &Message {
                    text: "request".into(),
                },
            )
            .unwrap();
            read_frame::<_, Message>(&mut connection).unwrap()
        });

        let mut connection = server.accept().unwrap();
        let request: Message = read_frame(&mut connection).unwrap();
        assert_eq!(request.text, "request");
        write_frame(
            &mut connection,
            &Message {
                text: "response".into(),
            },
        )
        .unwrap();

        assert_eq!(client.join().unwrap().text, "response");
        assert!(current_user_pipe_name().unwrap().contains("S-1-"));
    }
}
