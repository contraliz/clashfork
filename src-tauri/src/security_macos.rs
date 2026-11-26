use anyhow::{Context, Result};
use clashfork_service_ipc::{OwnerCredentials, OwnerIdentity};
use std::{
    fs::OpenOptions,
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

const KEYCHAIN_SERVICE: &str = "io.clashfork.desktop";
const KEYCHAIN_ACCOUNT: &str = "protected-local-credentials-v1";

pub fn current_owner_credentials(app_data_root: &Path) -> Result<OwnerCredentials> {
    use std::os::unix::fs::MetadataExt as _;
    let root = std::fs::canonicalize(app_data_root)
        .context("failed to canonicalize the private application data directory")?;
    let metadata = std::fs::metadata(&root)?;
    let uid = nix::unistd::getuid().as_raw();
    let gid = nix::unistd::getgid().as_raw();
    anyhow::ensure!(
        metadata.uid() == uid,
        "application data directory belongs to a different user"
    );
    Ok(OwnerCredentials {
        identity: OwnerIdentity::Unix { uid, gid },
        app_data_dir: root.to_string_lossy().into_owned(),
        token: None,
    })
}

pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn write_secrets_file(path: &Path, plaintext: &[u8]) -> Result<()> {
    let value = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, plaintext);
    keychain_entry()?
        .set_password(&value)
        .context("failed to save credentials in the macOS Keychain")?;
    // Preserve the existing store's presence check without placing credential bytes on disk.
    write_private_file(path, b"credentials are stored in the macOS Keychain")
}

pub fn read_secrets_file(_path: &Path) -> Result<Vec<u8>> {
    let value = keychain_entry()?
        .get_password()
        .context("failed to read credentials from the macOS Keychain")?;
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, value)
        .context("the macOS Keychain credential record is malformed")
}

fn keychain_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .context("could not access the macOS Keychain")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_files_have_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private");
        write_private_file(&path, b"fixture").unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
