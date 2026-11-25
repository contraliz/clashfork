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
    pub protocol_info: Option<ProtocolInfo>,
}

pub async fn probe() -> ServiceProbe {
    if !clashfork_service_ipc::is_ipc_path_exists() {
        return ServiceProbe {
            available: false,
            message: Some(SERVICE_UNAVAILABLE.to_owned()),
            protocol_info: None,
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
                    protocol_info: response.data,
                }
            } else {
                ServiceProbe {
                    available: false,
                    message: Some(SERVICE_NEEDS_REPAIR.to_owned()),
                    protocol_info: None,
                }
            }
        }
        _ => ServiceProbe {
            available: false,
            message: Some(SERVICE_UNAVAILABLE.to_owned()),
            protocol_info: None,
        },
    }
}

pub async fn already_owned_session(
    paths: &AppPaths,
    secrets: &mut SecretStore,
) -> Result<Option<String>> {
    let credentials = security::current_owner_credentials(&paths.data_root)?;
    let response = clashfork_service_ipc::get_status(&credentials)
        .await
        .context("the previous service session could not be inspected")?;
    if response.code != 0 {
        bail!(safe_service_message(response.code, &response.message));
    }
    let Some(status) = response.data else {
        bail!("the service did not return previous-session status");
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
    let Some(proof) = proof_for_active_session(status.active_generation, &stored.token) else {
        return Ok(Some(
            "A previous Clashfork VPN session is active, but its generation could not be confirmed. Reinstall Clashfork Service to clear it safely."
                .to_owned(),
        ));
    };
    secrets.last_session = Some(stored_session(&proof.token, proof.generation));
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

fn proof_for_active_session(generation: Option<u64>, token: &str) -> Option<OwnerSessionProof> {
    Some(OwnerSessionProof {
        generation: generation?,
        token: token.to_owned(),
    })
}

pub async fn start(
    paths: &AppPaths,
    yaml: String,
    controller: &ControllerConfig,
    session_token: String,
    info: ProtocolInfo,
) -> Result<(StartClashResult, OwnerSessionProof, String)> {
    let core_path = verify_installed_core(paths).await?;
    let credentials = security::current_owner_credentials(&paths.data_root)?;
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
            "The current system user is not authorized"
        }
        x if x == ServiceErrorCode::NotActive as u16 => "The VPN session is no longer active",
        x if x == ServiceErrorCode::InvalidInstallLocation as u16 => {
            "The installed Mihomo core is not approved; reinstall Clashfork"
        }
        x if x == ServiceErrorCode::InvalidRuntimeAsset as u16 => {
            "The generated runtime was rejected"
        }
        x if x == ServiceErrorCode::OwnerSwitchFailed as u16 => {
            "Another system user owns the VPN service"
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

const CONTROLLER_READY_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROLLER_READY_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn wait_ready_mode(
    controller: &crate::controller::CoreController,
    expected_mode: &str,
) -> Result<()> {
    wait_ready_config_with_timeout(controller, expected_mode, None, CONTROLLER_READY_TIMEOUT)
        .await
        .map(|_| ())
}

pub async fn wait_ready_config(
    controller: &crate::controller::CoreController,
    expected_mode: &str,
    expected_tun: bool,
) -> Result<crate::controller::ConfigStatus> {
    wait_ready_config_with_timeout(
        controller,
        expected_mode,
        Some(expected_tun),
        CONTROLLER_READY_TIMEOUT,
    )
    .await
}

async fn wait_ready_config_with_timeout(
    controller: &crate::controller::CoreController,
    expected_mode: &str,
    expected_tun: Option<bool>,
    timeout: Duration,
) -> Result<crate::controller::ConfigStatus> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            bail!(readiness_error(expected_mode, expected_tun));
        }
        let request_timeout = remaining.min(CONTROLLER_READY_REQUEST_TIMEOUT);
        if let Ok(Ok(status)) =
            tokio::time::timeout(request_timeout, controller.config_status()).await
            && status.mode.eq_ignore_ascii_case(expected_mode)
            && expected_tun.is_none_or(|expected| status.tun_enabled == Some(expected))
        {
            return Ok(status);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            bail!(readiness_error(expected_mode, expected_tun));
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(150))).await;
    }
}

fn readiness_error(expected_mode: &str, expected_tun: Option<bool>) -> String {
    match expected_tun {
        Some(true) => format!(
            "Mihomo did not report the selected {expected_mode} mode with TUN enabled. The VPN session was cleaned up."
        ),
        Some(false) => {
            "Mihomo did not confirm TUN disabled in its selected mode. The VPN session was cleaned up."
                .to_owned()
        }
        None => "Mihomo did not become ready in the selected mode. The TUN session was cleaned up."
            .to_owned(),
    }
}

pub fn open_credentials(paths: &AppPaths) -> Result<OwnerCredentials> {
    security::current_owner_credentials(&paths.data_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn pending_session_token_recovers_the_service_generation_after_a_lost_start_reply() {
        let proof = proof_for_active_session(Some(42), "fixture-session-token").unwrap();
        assert_eq!(proof.generation, 42);
        assert_eq!(proof.token, "fixture-session-token");
        assert!(proof_for_active_session(None, "fixture-session-token").is_none());
    }

    async fn respond_mode(stream: &mut tokio::net::TcpStream, mode: &str) {
        let mut request = [0u8; 1024];
        let length = stream.read(&mut request).await.unwrap();
        assert!(
            String::from_utf8_lossy(&request[..length]).starts_with("GET /configs HTTP/1.1"),
            "readiness should use the config endpoint that reports mode"
        );
        let body = format!(r#"{{"mode":"{mode}","tun":{{"enable":false}}}}"#);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn readiness_waits_for_selected_mode_using_controller_config_endpoint() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            for mode in ["rule", "global"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                respond_mode(&mut stream, mode).await;
            }
        });
        let controller = crate::controller::CoreController::new(
            port,
            "fixture-token".to_owned(),
            reqwest::Client::new(),
        );

        wait_ready_config_with_timeout(&controller, "global", Some(false), Duration::from_secs(2))
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn readiness_http_request_obeys_overall_deadline() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let controller = crate::controller::CoreController::new(
            port,
            "fixture-token".to_owned(),
            reqwest::Client::new(),
        );

        let result = wait_ready_config_with_timeout(
            &controller,
            "rule",
            Some(true),
            Duration::from_millis(100),
        )
        .await;
        assert!(result.is_err());
        server.abort();
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_service_tun_smoke {
    use super::*;
    use std::{net::TcpListener, process::Command, time::Duration};

    fn free_port() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

    fn fixture_yaml(mode: &str, controller_port: u16, dns_port: u16, secret: &str) -> String {
        let (groups, route) = if mode == "global" {
            (
                "proxy-groups:\n  - name: GLOBAL\n    type: select\n    proxies:\n      - DIRECT\n",
                "MATCH,GLOBAL",
            )
        } else {
            ("", "MATCH,DIRECT")
        };
        format!(
            "mixed-port: 0\nallow-lan: false\nmode: {mode}\nlog-level: warning\nipv6: false\nfind-process-mode: always\nunified-delay: true\nexternal-controller: 127.0.0.1:{controller_port}\nsecret: {secret}\nprofile:\n  store-selected: false\n  store-fake-ip: false\ntun:\n  enable: true\n  stack: mixed\n  device: {}\n  auto-route: true\n  auto-detect-interface: true\n  strict-route: true\n  dns-hijack:\n    - any:53\n    - tcp://any:53\ndns:\n  enable: true\n  listen: 127.0.0.1:{dns_port}\n  ipv6: false\n  enhanced-mode: fake-ip\n  fake-ip-range: 198.18.0.1/16\n  nameserver:\n    - https://1.1.1.1/dns-query\n    - https://223.5.5.5/dns-query\n{groups}rules:\n  - {route}\n",
            crate::rules::TUN_DEVICE
        )
    }

    fn route_interface(address: &str) -> Option<String> {
        let output = Command::new("/sbin/route")
            .args(["-n", "get", address])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout)
            .ok()?
            .lines()
            .find_map(|line| {
                let (key, value) = line.trim().split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case("interface")
                    .then(|| value.trim().to_owned())
            })
    }

    async fn fixed_https_probe() -> Result<()> {
        let response = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(15))
            .build()?
            .get("https://www.gstatic.com/generate_204")
            .send()
            .await?;
        anyhow::ensure!(response.status() == reqwest::StatusCode::NO_CONTENT);
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a disposable macOS runner with the Clashfork launchd service installed"]
    async fn stock_mihomo_authenticated_service_runs_tun_in_rule_and_global_then_stops()
    -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let paths = AppPaths::at(temporary.path().join("Clashfork"))?;
        let credentials = security::current_owner_credentials(&paths.data_root)?;
        let controller_config = ControllerConfig {
            port: free_port(),
            secret: new_session_token(),
            dns_port: free_port(),
        };
        let token = new_session_token();
        security::write_private_file(
            &paths.secrets_file,
            &serde_json::to_vec(&stored_session(&token, 0))?,
        )?;
        let protocol = probe()
            .await
            .protocol_info
            .context("installed service did not answer its protocol probe")?;
        let controller = CoreController::new(
            controller_config.port,
            controller_config.secret.clone(),
            Client::new(),
        );
        let mut proof = None;

        let exercise = async {
            let (started, session, _) = start(
                &paths,
                fixture_yaml(
                    "rule",
                    controller_config.port,
                    controller_config.dns_port,
                    &controller_config.secret,
                ),
                &controller_config,
                token.clone(),
                protocol,
            )
            .await?;
            proof = Some(session.clone());
            security::write_private_file(
                &paths.secrets_file,
                &serde_json::to_vec(&stored_session(&session.token, session.generation))?,
            )?;
            anyhow::ensure!(started.session.generation == session.generation);
            wait_ready_config(&controller, "rule", true).await?;
            for address in ["1.1.1.1", "8.8.8.8"] {
                anyhow::ensure!(
                    route_interface(address).as_deref() == Some(crate::rules::TUN_DEVICE),
                    "the public best route for {address} did not select the requested utun device"
                );
            }
            fixed_https_probe().await?;

            let staged = stage(
                &paths,
                fixture_yaml(
                    "global",
                    controller_config.port,
                    controller_config.dns_port,
                    &controller_config.secret,
                ),
                &session,
            )
            .await?;
            let Staged::Ready(config_path) = staged else {
                anyhow::bail!("the service requested an untested core restart during a mode change")
            };
            controller.reload(&config_path).await?;
            wait_ready_config(&controller, "global", true).await?;
            fixed_https_probe().await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;

        if proof.is_none()
            && let Ok(response) = clashfork_service_ipc::get_status(&credentials).await
            && let Some(status) = response.data
            && status.is_active
            && let Some(generation) = status.active_generation
        {
            proof = Some(OwnerSessionProof {
                generation,
                token: token.clone(),
            });
        }

        let cleanup = match proof.as_ref() {
            Some(session) => stop(&paths, session).await,
            None => Ok(()),
        };
        if cleanup.is_err() {
            let retained = temporary.keep();
            anyhow::bail!(
                "TUN smoke cleanup failed; private session proof was retained at {:?}",
                retained
            );
        }
        let status = clashfork_service_ipc::get_status(&credentials).await?;
        anyhow::ensure!(
            status.code == 0 && status.data.is_some_and(|snapshot| !snapshot.is_active),
            "the service did not confirm that its TUN session stopped"
        );
        for address in ["1.1.1.1", "8.8.8.8"] {
            let interface = route_interface(address).with_context(|| {
                format!("the route API could not confirm cleanup for {address}")
            })?;
            anyhow::ensure!(
                interface != crate::rules::TUN_DEVICE,
                "the public route for {address} remained on the Clashfork utun after stop"
            );
        }
        exercise?;
        Ok(())
    }
}
