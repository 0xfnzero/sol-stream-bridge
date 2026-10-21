use super::super::access::ManagedState;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub const MAX_STATE_BYTES: u64 = 1024 * 1024;
pub fn bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.len() > MAX_STATE_BYTES {
        bail!("dashboard state must be a regular file of at most 1 MiB");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("dashboard state contains keys; set its permissions to 600");
        }
    }
    Ok(Some(fs::read(path)?))
}
pub fn read(path: &Path) -> Result<Option<ManagedState>> {
    bytes(path)?
        .map(|bytes| {
            let text = std::str::from_utf8(&bytes).context("invalid dashboard state encoding")?;
            toml::from_str(text).map_err(|_| {
                anyhow::anyhow!("invalid dashboard state; refusing to fall back to config keys")
            })
        })
        .transpose()
}
pub fn fingerprint(bytes: &Option<Vec<u8>>) -> Option<[u8; 32]> {
    bytes.as_ref().map(|b| Sha256::digest(b).into())
}
pub struct Writer {
    pub path: PathBuf,
    pub expected: Option<[u8; 32]>,
    // Held for the server's lifetime; multiple writers on one state file fail closed.
    _lock: File,
}
impl Writer {
    pub fn open(path: PathBuf) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let lock_path = path.with_extension("toml.lock");
        if fs::symlink_metadata(&lock_path).is_ok_and(|m| !m.is_file()) {
            bail!("dashboard state lock must be a regular file");
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(lock_path)?;
        lock.try_lock()
            .context("dashboard state is already managed by another process")?;
        Ok(Self {
            expected: fingerprint(&bytes(&path)?),
            path,
            _lock: lock,
        })
    }
}
pub fn save(path: &Path, expected: Option<[u8; 32]>, contents: &str) -> Result<[u8; 32]> {
    if fingerprint(&bytes(path)?) != expected {
        bail!("dashboard state changed outside this process; restart before editing");
    }
    if contents.len() as u64 > MAX_STATE_BYTES {
        bail!("dashboard state exceeds 1 MiB");
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(contents.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    // Rename is the commit point. Apply runtime state even if directory syncing
    // is unsupported; otherwise disk and runtime could disagree after success.
    if File::open(parent)
        .and_then(|directory| directory.sync_all())
        .is_err()
    {
        tracing::warn!("dashboard state saved; directory durability could not be confirmed");
    }
    Ok(Sha256::digest(contents.as_bytes()).into())
}
