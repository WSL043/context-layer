use std::io::{self, Read, Write};

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("frame length {actual} exceeds maximum {maximum}")]
    TooLarge { actual: usize, maximum: usize },
    #[error("invalid JSON frame: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, FrameError> {
    let mut length_bytes = [0u8; 4];
    reader.read_exact(&mut length_bytes)?;
    let length = u32::from_le_bytes(length_bytes) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            actual: length,
            maximum: MAX_FRAME_BYTES,
        });
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    Ok(serde_json::from_slice(&payload)?)
}

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), FrameError> {
    let payload = serde_json::to_vec(value)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            actual: payload.len(),
            maximum: MAX_FRAME_BYTES,
        });
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

#[cfg(windows)]
mod windows_pipe;

#[cfg(windows)]
pub use windows_pipe::{
    NamedPipeClient, NamedPipeConnection, NamedPipeServer, current_user_pipe_name,
};

#[cfg(not(windows))]
mod unsupported {
    use std::io::{self, Read, Write};

    fn error() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows named pipes are only available on Windows",
        )
    }

    pub fn current_user_pipe_name() -> io::Result<String> {
        Err(error())
    }

    pub struct NamedPipeServer;

    impl NamedPipeServer {
        pub fn bind_current_user() -> io::Result<Self> {
            Err(error())
        }

        pub fn accept(self) -> io::Result<NamedPipeConnection> {
            Err(error())
        }
    }

    pub struct NamedPipeClient;

    impl NamedPipeClient {
        pub fn connect_current_user(_timeout_ms: u32) -> io::Result<NamedPipeConnection> {
            Err(error())
        }
    }

    pub struct NamedPipeConnection;

    impl Read for NamedPipeConnection {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(error())
        }
    }

    impl Write for NamedPipeConnection {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(error())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(error())
        }
    }
}

#[cfg(not(windows))]
pub use unsupported::{
    NamedPipeClient, NamedPipeConnection, NamedPipeServer, current_user_pipe_name,
};

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Message {
        value: String,
    }

    #[test]
    fn framed_json_round_trips() {
        let expected = Message {
            value: "browser event".into(),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();

        let actual: Message = read_frame(&mut bytes.as_slice()).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocation() {
        let bytes = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes().to_vec();
        let error = read_frame::<_, Message>(&mut bytes.as_slice()).unwrap_err();
        assert!(matches!(error, FrameError::TooLarge { .. }));
    }
}
