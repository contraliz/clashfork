use anyhow::{Context, Result, bail};
use std::{
    env,
    path::{Path, PathBuf},
};

pub const DATA_DIR_NAME: &str = "Clashfork";
pub const SERVICE_SLUG: &str = "clashfork-service";
#[cfg(windows)]
pub const CORE_FILE_NAME: &str = "clashfork-core.exe";
#[cfg(target_os = "macos")]
pub const CORE_FILE_NAME: &str = "clashfork-core";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_root: PathBuf,
    pub runtime_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub state_file: PathBuf,
    pub secrets_file: PathBuf,
    pub history_file: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let root = if let Some(root) = env::var_os("CLASHFORK_DATA_DIR") {
            PathBuf::from(root)
        } else {
            #[cfg(windows)]
            let base = env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .context("LOCALAPPDATA is unavailable")?;
            #[cfg(target_os = "macos")]
            let base = env::var_os("HOME")
                .map(PathBuf::from)
                .context("HOME is unavailable")?
                .join("Library")
                .join("Application Support");
            base.join(DATA_DIR_NAME)
        };
        Self::at(root)
    }

    pub fn at(root: PathBuf) -> Result<Self> {
        let paths = Self {
            runtime_dir: root.join("runtime"),
            logs_dir: root.join("logs"),
            state_file: root.join("state.json"),
            secrets_file: root.join("secrets.dat"),
            history_file: root.join("history.sqlite3"),
            data_root: root,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for path in [&paths.data_root, &paths.runtime_dir, &paths.logs_dir] {
                std::fs::create_dir_all(path)?;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        #[cfg(windows)]
        std::fs::create_dir_all(&paths.data_root)
            .with_context(|| format!("failed to create {:?}", paths.data_root))?;
        #[cfg(windows)]
        std::fs::create_dir_all(&paths.runtime_dir)
            .with_context(|| format!("failed to create {:?}", paths.runtime_dir))?;
        #[cfg(windows)]
        std::fs::create_dir_all(&paths.logs_dir)
            .with_context(|| format!("failed to create {:?}", paths.logs_dir))?;
        Ok(paths)
    }

    pub fn installed_core(&self) -> Result<PathBuf> {
        #[cfg(windows)]
        {
            let program_data = env::var_os("ProgramData").context("ProgramData is unavailable")?;
            Ok(PathBuf::from(program_data)
                .join(SERVICE_SLUG)
                .join("cores")
                .join(CORE_FILE_NAME))
        }
        #[cfg(target_os = "macos")]
        bail!("the service owns its protected core installation directory")
    }

    pub fn bundled_core(&self) -> Result<PathBuf> {
        if let Some(value) = env::var_os("CLASHFORK_CORE_PATH") {
            let path = PathBuf::from(value);
            if path.is_file() {
                return Ok(path);
            }
        }
        let exe = env::current_exe().context("failed to locate the Clashfork executable")?;
        let exe_dir = exe
            .parent()
            .context("the Clashfork executable has no parent")?;
        let candidates = [
            exe_dir.join("resources").join(CORE_FILE_NAME),
            exe_dir.join(CORE_FILE_NAME),
            exe_dir.join("..").join("resources").join(CORE_FILE_NAME),
            exe_dir
                .join("..")
                .join("Resources")
                .join("resources")
                .join(CORE_FILE_NAME),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join(CORE_FILE_NAME),
        ];
        for candidate in candidates {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        bail!("the bundled Mihomo core is missing")
    }

    pub fn service_install_helper(&self) -> Result<PathBuf> {
        #[cfg(windows)]
        {
            self.resource("clashfork-service-install.exe")
        }
        #[cfg(target_os = "macos")]
        {
            self.resource("clashfork-service-install")
        }
    }

    pub fn service_uninstall_helper(&self) -> Result<PathBuf> {
        #[cfg(windows)]
        {
            self.resource("clashfork-service-uninstall.exe")
        }
        #[cfg(target_os = "macos")]
        {
            self.resource("clashfork-service-uninstall")
        }
    }

    #[cfg(target_os = "macos")]
    pub fn install_or_repair_macos_service(&self) -> Result<()> {
        let helper = self.service_install_helper()?;
        let core = self.bundled_core()?;
        let output = std::process::Command::new(helper)
            .args(["--prepare-install", "--core", "clashfork-core"])
            .arg(core)
            .args([
                "--ensure",
                "--prompt",
                "Install or repair Clashfork's protected VPN service",
            ])
            .output()
            .context("could not start the macOS service installer")?;
        if !output.status.success() {
            let details = String::from_utf8_lossy(&output.stderr);
            bail!(
                "Clashfork service installation did not complete: {}",
                details.trim()
            );
        }
        Ok(())
    }

    fn resource(&self, name: &str) -> Result<PathBuf> {
        let exe = env::current_exe().context("failed to locate the Clashfork executable")?;
        let exe_dir = exe
            .parent()
            .context("the Clashfork executable has no parent")?;
        let candidates = [
            exe_dir.join("resources").join(name),
            exe_dir.join(name),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join(name),
            exe_dir
                .join("..")
                .join("Resources")
                .join("resources")
                .join(name),
            exe_dir.join("..").join("Resources").join(name),
        ];
        candidates
            .into_iter()
            .find(|path| path.is_file())
            .with_context(|| format!("the bundled service helper {name} is missing"))
    }

    pub fn ephemeral_config(&self, name: &str) -> PathBuf {
        self.runtime_dir.join(name)
    }

    pub fn cleanup_runtime(&self) {
        if let Ok(entries) = std::fs::read_dir(&self.runtime_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    pub fn redact_path(path: &Path) -> String {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[internal path]".to_owned())
    }
}

pub fn program_data_service_root() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("ProgramData")
            .map(PathBuf::from)
            .map(|path| path.join(SERVICE_SLUG))
            .context("ProgramData is unavailable")
    }
    #[cfg(target_os = "macos")]
    bail!("the service owns its protected installation directory")
}

pub fn private_temp_file(directory: &Path, label: &str, bytes: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let name = format!("{label}-{}.yaml", uuid::Uuid::new_v4());
    let path = directory.join(name);
    crate::security::write_private_file(&path, bytes)?;
    Ok(path)
}
