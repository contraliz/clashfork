use fs2::FileExt as _;
use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt, sync::OnceLock};

static INSTANCE_LOCK: OnceLock<std::fs::File> = OnceLock::new();

pub fn acquire() -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let directory = std::path::PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("Clashfork");
    if std::fs::create_dir_all(&directory).is_err() {
        return false;
    }
    let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(directory.join(".instance-lock"))
    else {
        return false;
    };
    if file.try_lock_exclusive().is_err() {
        return false;
    }
    INSTANCE_LOCK.set(file).is_ok()
}
