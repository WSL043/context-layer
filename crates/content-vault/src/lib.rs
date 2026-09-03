use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredContent {
    pub sha256: String,
    pub byte_length: u64,
    pub path: PathBuf,
    pub duplicate: bool,
}

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("content vault I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("content hash path requires a 64-character lowercase SHA-256 digest")]
    InvalidDigest,
}

pub struct ContentVault {
    root: PathBuf,
}

impl ContentVault {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, VaultError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn put_bytes(&self, bytes: &[u8]) -> Result<StoredContent, VaultError> {
        let sha256 = hex_sha256(bytes);
        let final_path = self.path_for_digest(&sha256)?;

        if final_path.exists() {
            return Ok(StoredContent {
                sha256,
                byte_length: bytes.len() as u64,
                path: final_path,
                duplicate: true,
            });
        }

        let parent = final_path.parent().expect("digest path always has a parent");
        fs::create_dir_all(parent)?;

        let temp_path = parent.join(format!(".{sha256}.tmp-{}", Uuid::now_v7()));
        let mut temp = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        temp.write_all(bytes)?;
        temp.sync_all()?;
        drop(temp);

        match fs::rename(&temp_path, &final_path) {
            Ok(()) => {}
            Err(_) if final_path.exists() => {
                let _ = fs::remove_file(&temp_path);
                return Ok(StoredContent {
                    sha256,
                    byte_length: bytes.len() as u64,
                    path: final_path,
                    duplicate: true,
                });
            }
            Err(error) => {
                let _ = fs::remove_file(&temp_path);
                return Err(error.into());
            }
        }

        Ok(StoredContent {
            sha256,
            byte_length: bytes.len() as u64,
            path: final_path,
            duplicate: false,
        })
    }

    pub fn path_for_digest(&self, digest: &str) -> Result<PathBuf, VaultError> {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(VaultError::InvalidDigest);
        }

        Ok(self
            .root
            .join(&digest[0..2])
            .join(&digest[2..4])
            .join(digest))
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn temp_vault_root() -> PathBuf {
        std::env::temp_dir().join(format!("context-vault-test-{}", Uuid::now_v7()))
    }

    #[test]
    fn stores_content_by_sha256_and_deduplicates_replays() {
        let root = temp_vault_root();
        let vault = ContentVault::open(&root).unwrap();

        let first = vault.put_bytes(b"personal context").unwrap();
        let second = vault.put_bytes(b"personal context").unwrap();

        assert!(!first.duplicate);
        assert!(second.duplicate);
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(fs::read(&first.path).unwrap(), b"personal context");
        assert!(first.path.starts_with(&root));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_style_replay_gets_same_content_path() {
        let root = temp_vault_root();
        let vault = ContentVault::open(&root).unwrap();

        let first = vault.put_bytes(b"same bytes").unwrap();
        let replay = vault.put_bytes(b"same bytes").unwrap();

        assert_eq!(first.path, replay.path);
        assert!(replay.duplicate);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_non_sha256_digest_paths() {
        let root = temp_vault_root();
        let vault = ContentVault::open(&root).unwrap();
        assert!(matches!(
            vault.path_for_digest("../escape"),
            Err(VaultError::InvalidDigest)
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
