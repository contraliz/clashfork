use crate::{
    diagnostics::redact,
    models::{SecretStore, StoredSession},
    paths::AppPaths,
    rules::ControllerConfig,
    security,
};
use anyhow::{Context, Result, bail};
use clashfork_service_ipc::{
    OwnerCredentials, OwnerSessionProof, ProtocolInfo, RuntimeBundle, ServiceErrorCode,
    StageRuntimeOutcome, StartClashRequest, StartClashResult,
};
use sha2::{Digest as _, Sha256};
use std::path::Path;
use std::time::Duration;

pub const SERVICE_UNAVAILABLE: &str =
    "Clashfork Service is not available. Reinstall Clashfork to install or repair the service.";
pub const SERVICE_NEEDS_REPAIR: &str =
    "Clashfork Service needs repair or a matching update. Reinstall Clashfork.";
pub const CORE_MISSING: &str =
    "Clashfork Service is running, but its approved Mihomo core is missing. Reinstall Clashfork.";

#[derive(Debug, Clone)]
pub struct ServiceProbe {
    pub available: bool,
    pub message: Option<String>,
}

pub async fn probe() -> ServiceProbe {
    if !clashfork_service_ipc::is_ipc_path_exists() {
        return ServiceProbe {
            available: false,
            message: Some(SERVICE_UNAVAILABLE.to_owned()),
        };
    }
    match clashfork_service_ipc::get_version().await {
        Ok(response) if response.code == 0 => {
            let compatible = response.data.as_ref().is_some_and(|info| {
                info.supports_client(
                    clashfork_service_ipc::ProtocolVersion::current(),
                    clashfork_service_ipc::MIN_REQUIRED_SERVICE_REVISION,
                )
            });
            if compatible {
                ServiceProbe {
                    available: true,
                    message: None,
                }
            } else {
                ServiceProbe {
                    available: false,
                    message: Some(SERVICE_NEEDS_REPAIR.to_owned()),
                }
            }
        }
        _ => ServiceProbe {
            available: false,
            message: Some(SERVICE_UNAVAILABLE.to_owned()),
        },
    }
}

pub async fn protocol_info() -> Result<ProtocolInfo> {
    let response = clashfork_service_ipc::get_version()
        .await
        .context(SERVICE_UNAVAILABLE)?;
    if response.code != 0 {
        bail!(SERVICE_NEEDS_REPAIR);
    }
    response.data.context(SERVICE_NEEDS_REPAIR)
}

pub async fn already_owned_session(
    paths: &AppPaths,
    secrets: &mut SecretStore,
) -> Result<Option<String>> {
    let credentials = security::current_owner_credentials(&paths.data_root)?;
    let response = match clashfork_service_ipc::get_status(&credentials).await {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    let Some(status) = response.data else {
        return Ok(None);
    };
    if !status.is_active {
        secrets.last_session = None;
        return Ok(None);
    }
    let Some(stored) = secrets.last_session.clone() else {
        return Ok(Some(
            "A previous Clashfork VPN session is still owned by the service, but its session proof is unavailable. Reinstall Clashfork Service to clear it."
                .to_owned(),
        ));
    };
    let proof = OwnerSessionProof {
        generation: stored.generation,
        token: stored.token,
    };
    match clashfork_service_ipc::stop_clash(&credentials, &proof).await {
        Ok(response) if response.code == 0 => {
            secrets.last_session = None;
            Ok(None)
        }
        Ok(response) => Ok(Some(safe_service_message(response.code, &response.message))),
        Err(_) => Ok(Some(
            "The previous Clashfork VPN session could not be stopped; reinstall Clashfork Service if the TUN adapter remains present."
                .to_owned(),
        )),
    }
}

pub async fn start(
    paths: &AppPaths,
    yaml: String,
    controller: &ControllerConfig,
    session_token: String,
) -> Result<(StartClashResult, OwnerSessionProof, String)> {
    let core_path = verify_installed_core(paths).await?;
    let credentials = security::current_owner_credentials(&paths.data_root)?;
    let info = protocol_info().await?;
    if !info.supports_client(
        clashfork_service_ipc::ProtocolVersion::current(),
        clashfork_service_ipc::MIN_REQUIRED_SERVICE_REVISION,
    ) {
        bail!(SERVICE_NEEDS_REPAIR);
    }
    let _ = controller.dns_port;
    let request = StartClashRequest {
        runtime: RuntimeBundle {
            yaml,
            assets: Vec::new(),
            remote_providers: Vec::new(),
            core_path: core_path.to_string_lossy().into_owned(),
        },
        proposed_session_token: session_token.clone(),
        macos_proxy: None,
    };
    let response = clashfork_service_ipc::start_clash(&credentials, &request)
        .await
        .context(SERVICE_UNAVAILABLE)?;
    if response.code != 0 {
        bail!(safe_service_message(response.code, &response.message));
    }
    let result = response
        .data
        .context("Clashfork Service did not return a session.")?;
    Ok((
        StartClashResult {
            session: result.session.clone(),
            proxy_outcome: result.proxy_outcome,
        },
        OwnerSessionProof {
            generation: result.session.generation,
            token: session_token,
        },
        core_path.to_string_lossy().into_owned(),
    ))
}

pub enum Staged {
    Ready(String),
    Restart,
}

pub async fn stage(paths: &AppPaths, yaml: String, session: &OwnerSessionProof) -> Result<Staged> {
    let core_path = verify_installed_core(paths).await?;
    let credentials = security::current_owner_credentials(&paths.data_root)?;
    let runtime = RuntimeBundle {
        yaml,
        assets: Vec::new(),
        remote_providers: Vec::new(),
        core_path: core_path.to_string_lossy().into_owned(),
    };
    let response = clashfork_service_ipc::stage_runtime(&credentials, session, &runtime)
        .await
        .context(SERVICE_UNAVAILABLE)?;
    if response.code != 0 {
        bail!(safe_service_message(response.code, &response.message));
    }
    match response
        .data
        .context("Clashfork Service did not acknowledge staging.")?
    {
        StageRuntimeOutcome::Staged { config_path } => Ok(Staged::Ready(config_path)),
        StageRuntimeOutcome::RestartRequired { .. } => Ok(Staged::Restart),
    }
}

pub async fn stop(paths: &AppPaths, session: &OwnerSessionProof) -> Result<()> {
    let credentials = security::current_owner_credentials(&paths.data_root)?;
    let response = clashfork_service_ipc::stop_clash(&credentials, session)
        .await
        .context(SERVICE_UNAVAILABLE)?;
    if response.code == 0 || response.code == ServiceErrorCode::NotActive as u16 {
        return Ok(());
    }
    bail!(safe_service_message(response.code, &response.message))
}

pub async fn verify_installed_core(paths: &AppPaths) -> Result<std::path::PathBuf> {
    let bundled = paths.bundled_core()?;
    let expected = sha256(&bundled)?;
    let status =
        clashfork_service_ipc::inspect_installation(&[clashfork_service_ipc::CoreRequirement {
            name: crate::paths::CORE_FILE_NAME.to_owned(),
            sha256: Some(expected),
        }])
        .await
        .context(SERVICE_UNAVAILABLE)?;
    let Some(core) = status
        .cores
        .into_iter()
        .find(|core| core.name.eq_ignore_ascii_case(crate::paths::CORE_FILE_NAME))
    else {
        bail!(CORE_MISSING);
    };
    match core.availability {
        clashfork_service_ipc::CoreAvailability::Ready => Ok(bundled),
        clashfork_service_ipc::CoreAvailability::Missing => bail!(CORE_MISSING),
        clashfork_service_ipc::CoreAvailability::DigestMismatch => {
            bail!(
                "The approved Clashfork core does not match the bundled pinned Mihomo build. Reinstall Clashfork."
            )
        }
        clashfork_service_ipc::CoreAvailability::Rejected { .. } => {
            bail!("The installed Mihomo core is not approved; reinstall Clashfork.")
        }
    }
}

fn sha256(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).context("failed to read a core binary")?;
    let mut digest = Sha256::new();
    digest.update(&bytes);
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn new_session_token() -> String {
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn stored_session(token: &str, generation: u64) -> StoredSession {
    StoredSession {
        token: token.to_owned(),
        generation,
    }
}

pub fn safe_service_message(code: u16, message: &str) -> String {
    let category = match code {
        x if x == ServiceErrorCode::UnauthorizedOwner as u16 => {
            "The current Windows user is not authorized"
        }
        x if x == ServiceErrorCode::NotActive as u16 => "The VPN session is no longer active",
        x if x == ServiceErrorCode::InvalidInstallLocation as u16 => {
            "The installed Mihomo core is not approved; reinstall Clashfork"
        }
        x if x == ServiceErrorCode::InvalidRuntimeAsset as u16 => {
            "The generated runtime was rejected"
        }
        x if x == ServiceErrorCode::OwnerSwitchFailed as u16 => {
            "Another Windows user owns the VPN service"
        }
        x if x == ServiceErrorCode::ProtocolMismatch as u16 => "The service protocol is mismatched",
        x if x == ServiceErrorCode::StaleOwnerSession as u16 => {
            "The VPN session is stale; reconnect"
        }
        x if x == ServiceErrorCode::AppDataRootNotOwned as u16 => {
            "The application data folder has the wrong owner"
        }
        _ => "The Clashfork service rejected the request",
    };
    let detail = truncate(&redact(message), 240);
    if detail.is_empty() {
        format!("{category} (service code {code}).")
    } else {
        format!("{category} (service code {code}): {detail}")
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    value.chars().take(max).collect()
}

pub async fn wait_ready(controller: &crate::controller::CoreController) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if controller.version().await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("The Mihomo core did not become ready. The TUN session was cleaned up.");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub fn open_credentials(paths: &AppPaths) -> Result<OwnerCredentials> {
    security::current_owner_credentials(&paths.data_root)
}
