use crate::{
    controller::CoreController,
    diagnostics::Diagnostics,
    history::HistoryStore,
    models::{
        AppState, CoreStatus, CustomAppGroup, HistoryInput, HistoryReport, ImportPreview,
        LiveRates, LiveRoute, LiveSnapshot, Mode, NodeTestProgress, NodeTestSummary, NodeView,
        PendingImport, RouteClass, RuntimeView, SecretStore, ServiceView, Snapshot, SourceType,
        StoredNodeLatency,
    },
    paths::AppPaths,
    rules::{self, ControllerConfig},
    security, service_client, store,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use futures_util::StreamExt;
use parking_lot::Mutex;
use reqwest::Client;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

#[derive(Clone)]
pub struct CoreSession {
    session: clashfork_service_ipc::OwnerSessionProof,
    controller: CoreController,
    controller_config: ControllerConfig,
}

#[derive(Clone)]
pub struct ActiveCore {
    core: CoreSession,
    yaml: String,
    summary: String,
    mode: Mode,
    node_routes: BTreeMap<String, String>,
    node_fingerprints: BTreeMap<String, String>,
    match_groups: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct StandbyCore {
    core: CoreSession,
    yaml: String,
}

pub struct EngineState {
    status: CoreStatus,
    service_available: bool,
    service_message: Option<String>,
    last_error: Option<String>,
    active: Option<ActiveCore>,
    standby: Option<StandbyCore>,
    unresolved_session: Option<clashfork_service_ipc::OwnerSessionProof>,
    unresolved_owner: bool,
    generation: u64,
    competing_tunnel: bool,
    pending_reconnect_services: HashSet<String>,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            status: CoreStatus::Off,
            service_available: false,
            service_message: None,
            last_error: None,
            active: None,
            standby: None,
            unresolved_session: None,
            unresolved_owner: false,
            generation: 0,
            competing_tunnel: false,
            pending_reconnect_services: HashSet::new(),
        }
    }
}

#[derive(Default)]
pub struct LiveState {
    pub active: bool,
    pub available: bool,
    pub download_rate: u64,
    pub upload_rate: u64,
    pub proxy_download_rate: u64,
    pub proxy_upload_rate: u64,
    pub download_total: u64,
    pub upload_total: u64,
    pub connections: Vec<LiveRoute>,
    pub sampled_at: i64,
    last_totals: Option<(u64, u64)>,
    last_sample_at: Option<Instant>,
    flow_counters: HashMap<String, (u64, u64)>,
    generation: u64,
    gap_marked: bool,
}

pub struct AppRuntime {
    pub paths: AppPaths,
    pub http: Client,
    // Subscription redirects are followed manually so custom provider headers are dropped cross-origin.
    pub subscription_http: Client,
    pub state: Mutex<AppState>,
    pub secrets: Mutex<SecretStore>,
    pub previews: Mutex<HashMap<String, PendingImport>>,
    pub engine: Mutex<EngineState>,
    pub live: Mutex<LiveState>,
    pub history: Mutex<HistoryStore>,
    pub diagnostics: Diagnostics,
    pub core_logs: Mutex<std::collections::VecDeque<String>>,
    pub apply_gate: tokio::sync::Mutex<()>,
    node_test_control: tokio::sync::RwLock<()>,
    node_test_limit: tokio::sync::Semaphore,
    node_test_in_flight: Mutex<HashSet<String>>,
    node_test_cancel: std::sync::atomic::AtomicBool,
    pub sample_gate: tokio::sync::RwLock<()>,
    refresh_flights: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    pub app: AppHandle,
    pub quit_requested: std::sync::atomic::AtomicBool,
    pub generation: AtomicU64,
}

struct NodeTestFlight {
    runtime: Arc<AppRuntime>,
    node_id: String,
}

impl Drop for NodeTestFlight {
    fn drop(&mut self) {
        self.runtime
            .node_test_in_flight
            .lock()
            .remove(&self.node_id);
    }
}

impl AppRuntime {
    pub fn new(app: AppHandle) -> Result<Arc<Self>> {
        let paths = AppPaths::discover()?;
        paths.cleanup_runtime();
        let mut state = store::load_state(&paths)?;
        normalize_stored_service_errors(&mut state);
        let secrets = store::load_secrets(&paths)?;
        let history = HistoryStore::open(&paths.history_file)?;
        let http = Client::builder()
            .user_agent("Clashfork/1.0 (Windows)")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 4 {
                    attempt.stop()
                } else if attempt.url().scheme() != "https" {
                    attempt.error(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "redirects must remain HTTPS",
                    ))
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .context("failed to initialize the secure HTTP client")?;
        let subscription_http = crate::subscription::subscription_http_client()?;
        Ok(Arc::new(Self {
            paths,
            http,
            subscription_http,
            state: Mutex::new(state),
            secrets: Mutex::new(secrets),
            previews: Mutex::new(HashMap::new()),
            engine: Mutex::new(EngineState::default()),
            live: Mutex::new(LiveState::default()),
            history: Mutex::new(history),
            diagnostics: Diagnostics::default(),
            core_logs: Mutex::new(std::collections::VecDeque::new()),
            apply_gate: tokio::sync::Mutex::new(()),
            node_test_control: tokio::sync::RwLock::new(()),
            node_test_limit: tokio::sync::Semaphore::new(4),
            node_test_in_flight: Mutex::new(HashSet::new()),
            node_test_cancel: std::sync::atomic::AtomicBool::new(false),
            sample_gate: tokio::sync::RwLock::new(()),
            refresh_flights: Mutex::new(HashMap::new()),
            app,
            quit_requested: std::sync::atomic::AtomicBool::new(false),
            generation: AtomicU64::new(1),
        }))
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.state.lock().clone();
        recompute_counts(&mut state);
        {
            let secrets = self.secrets.lock();
            for node in &mut state.nodes {
                node.latency_ms = None;
                node.latency_measured_at_ms = None;
                if node.missing {
                    continue;
                }
                let Some((definition, record)) = secrets
                    .nodes
                    .get(&node.id)
                    .zip(secrets.node_latencies.get(&node.id))
                else {
                    continue;
                };
                if rules::node_definition_fingerprint(node, definition)
                    .is_ok_and(|fingerprint| fingerprint == record.definition_fingerprint)
                {
                    node.latency_ms = Some(record.latency_ms);
                    node.latency_measured_at_ms = Some(record.measured_at_ms);
                }
            }
        }
        let engine = self.engine.lock();
        let active = engine.active.as_ref();
        Snapshot {
            state,
            runtime: RuntimeView {
                vpn_active: (active.is_some()
                    && matches!(engine.status, CoreStatus::On | CoreStatus::Error))
                    || engine.unresolved_owner
                    || engine.unresolved_session.is_some(),
                core_status: engine.status,
                service_available: engine.service_available,
                service_message: engine.service_message.clone(),
                last_error: engine.last_error.clone(),
                active_summary: active.map(|active| active.summary.clone()).or_else(|| {
                    (engine.unresolved_owner || engine.unresolved_session.is_some())
                        .then(|| "Previous session needs attention".to_owned())
                }),
                competing_tunnel: engine.competing_tunnel,
                pending_reconnect_services: engine
                    .pending_reconnect_services
                    .iter()
                    .cloned()
                    .collect(),
            },
        }
    }

    pub async fn initialize(self: &Arc<Self>) {
        self.diagnostics.push(
            "app",
            "info",
            "Clashfork started with the VPN disconnected.",
        );
        let paths = self.paths.clone();
        {
            let mut history = self.history.lock();
            if let Err(error) = history.prune(Utc::now().timestamp()) {
                self.diagnostics.push(
                    "app",
                    "warn",
                    format!("History retention failed: {error:#}"),
                );
            }
        }
        let _ = paths;
        tauri::async_runtime::spawn(sampler_loop(self.clone()));
        tauri::async_runtime::spawn(startup_service_probe(self.clone()));
    }

    pub(crate) async fn probe_service(&self) {
        let probe = service_client::probe().await;
        {
            let mut engine = self.engine.lock();
            engine.service_available = probe.available;
            engine.service_message = probe.message;
        }
        self.emit_snapshot();
    }

    async fn prime_standby(
        &self,
        protocol_info: clashfork_service_ipc::ProtocolInfo,
    ) -> Result<()> {
        if self.quit_requested.load(Ordering::SeqCst) {
            return Ok(());
        }
        if self.engine.lock().standby.is_some() || self.engine.lock().active.is_some() {
            return Ok(());
        }
        let started_at = Instant::now();
        let controller_config = ControllerConfig {
            port: pick_port()?,
            secret: random_hex(32),
            dns_port: pick_port()?,
        };
        let yaml = rules::build_standby_yaml(&controller_config)?;
        let validation_started = Instant::now();
        self.validate_config(&yaml).await?;
        self.record_startup_stage(
            "inert standby validation",
            validation_started.elapsed(),
            true,
        );

        let session_token = service_client::new_session_token();
        {
            let mut secrets = self.secrets.lock();
            secrets.last_session = Some(service_client::stored_session(&session_token, 0));
            store::save_secrets(&self.paths, &secrets)
                .context("the warm-core stop proof could not be saved before service start")?;
        }
        let service_start_started = Instant::now();
        let (_, proof, _) = match service_client::start(
            &self.paths,
            yaml.clone(),
            &controller_config,
            session_token.clone(),
            protocol_info,
        )
        .await
        {
            Ok(started) => started,
            Err(error) => {
                self.reconcile_uncertain_start().await;
                return Err(error);
            }
        };
        self.record_startup_stage("warm core prime", service_start_started.elapsed(), true);
        let core = CoreSession {
            session: proof.clone(),
            controller: CoreController::new(
                controller_config.port,
                controller_config.secret.clone(),
                self.http.clone(),
            ),
            controller_config,
        };
        let save_result = {
            let mut secrets = self.secrets.lock();
            secrets.last_session = Some(service_client::stored_session(
                &session_token,
                proof.generation,
            ));
            store::save_secrets(&self.paths, &secrets)
        };
        if let Err(error) = save_result {
            if service_client::stop(&self.paths, &proof).await.is_ok() {
                let mut secrets = self.secrets.lock();
                secrets.last_session = None;
                let _ = store::save_secrets(&self.paths, &secrets);
            } else {
                let mut engine = self.engine.lock();
                engine.standby = Some(StandbyCore {
                    core: core.clone(),
                    yaml,
                });
                engine.status = CoreStatus::Error;
                engine.last_error = Some(
                    "The warm core started, but its session proof could not be saved safely. Try Disconnect or Quit.".to_owned(),
                );
            }
            return Err(error.context("the warm core session could not be recorded safely"));
        }

        let controller_ready =
            service_client::wait_ready_config(&core.controller, "rule", false).await;
        if let Err(error) = controller_ready {
            if service_client::stop(&self.paths, &proof).await.is_ok() {
                let mut secrets = self.secrets.lock();
                secrets.last_session = None;
                let _ = store::save_secrets(&self.paths, &secrets);
            } else {
                self.engine.lock().standby = Some(StandbyCore {
                    core: core.clone(),
                    yaml,
                });
                self.engine.lock().status = CoreStatus::Error;
                self.engine.lock().last_error = Some(error.to_string());
            }
            return Err(error.context("the warm core did not confirm TUN disabled"));
        }
        if !can_retain_warm_core(true, wait_for_tun_routes_disabled().await) {
            let error = anyhow::anyhow!(
                "Windows could not confirm that the warm core left public routes unchanged."
            );
            if service_client::stop(&self.paths, &proof).await.is_ok() {
                let mut secrets = self.secrets.lock();
                secrets.last_session = None;
                let _ = store::save_secrets(&self.paths, &secrets);
            } else {
                self.engine.lock().standby = Some(StandbyCore {
                    core: core.clone(),
                    yaml,
                });
                self.engine.lock().status = CoreStatus::Error;
                self.engine.lock().last_error = Some(error.to_string());
            }
            return Err(error);
        }

        self.engine.lock().standby = Some(StandbyCore { core, yaml });
        self.diagnostics.push(
            "app",
            "info",
            format!(
                "Warm core is ready with TUN and traffic listeners disabled ({} ms).",
                started_at.elapsed().as_millis()
            ),
        );
        Ok(())
    }

    fn refresh_tunnel_advisory(&self) {
        let detected = crate::network::active_competing_tunnel();
        let was_detected = {
            let mut engine = self.engine.lock();
            let previous = engine.competing_tunnel;
            engine.competing_tunnel = detected;
            previous
        };
        if detected && !was_detected {
            self.diagnostics.push(
                "app",
                "warn",
                "Another active tunnel adapter currently owns the selected outbound route. Clashfork has not changed system routing.",
            );
        }
    }

    pub fn emit_snapshot(&self) {
        let _ = self.app.emit("snapshot-changed", self.snapshot());
    }

    pub(crate) async fn set_vpn(self: &Arc<Self>, enabled: bool) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        let result = if enabled {
            self.enable_vpn().await
        } else {
            self.disable_vpn().await
        };
        self.emit_snapshot();
        crate::tray::refresh(self).ok();
        result?;
        Ok(self.snapshot())
    }

    pub async fn test_node(self: &Arc<Self>, node_id: &str) -> Result<Snapshot> {
        let _control = self.node_test_control.read().await;
        self.test_node_inner(node_id.to_owned()).await?;
        Ok(self.snapshot())
    }

    pub async fn test_all_nodes(self: &Arc<Self>) -> Result<NodeTestSummary> {
        let already_testing = self.node_test_in_flight.lock().clone();
        let _control = self.node_test_control.write().await;
        self.node_test_cancel.store(false, Ordering::SeqCst);
        if !self.has_connected_node_test_core() {
            bail!("Connect Clashfork before testing nodes.");
        }
        let node_ids: Vec<String> = self
            .state
            .lock()
            .nodes
            .iter()
            .filter(|node| !node.missing && !already_testing.contains(&node.id))
            .map(|node| node.id.clone())
            .collect();
        let total = node_ids.len();
        self.emit_node_test_progress(NodeTestProgress {
            total,
            completed: 0,
            responded: 0,
            failed: 0,
            node_id: None,
            latency_ms: None,
            finished: total == 0,
        });
        if total == 0 {
            return Ok(NodeTestSummary {
                total,
                completed: 0,
                responded: 0,
                failed: 0,
                cancelled: false,
            });
        }

        let counts = Arc::new(Mutex::new((0usize, 0usize, 0usize)));
        let runtime = Arc::clone(self);
        futures_util::stream::iter(node_ids)
            .for_each_concurrent(4, |node_id| {
                let runtime = Arc::clone(&runtime);
                let counts = Arc::clone(&counts);
                async move {
                    if runtime.node_test_cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    let result = runtime.test_node_inner(node_id.clone()).await;
                    let latency_ms = result.ok();
                    let (completed, responded, failed) = {
                        let mut counts = counts.lock();
                        counts.0 += 1;
                        if latency_ms.is_some() {
                            counts.1 += 1;
                        } else {
                            counts.2 += 1;
                        }
                        *counts
                    };
                    runtime.emit_node_test_progress(NodeTestProgress {
                        total,
                        completed,
                        responded,
                        failed,
                        node_id: Some(node_id),
                        latency_ms,
                        finished: false,
                    });
                }
            })
            .await;
        let (completed, responded, failed) = *counts.lock();
        let cancelled = self.node_test_cancel.swap(false, Ordering::SeqCst);
        self.emit_node_test_progress(NodeTestProgress {
            total,
            completed,
            responded,
            failed,
            node_id: None,
            latency_ms: None,
            finished: true,
        });
        Ok(NodeTestSummary {
            total,
            completed,
            responded,
            failed,
            cancelled,
        })
    }

    pub fn cancel_node_test_sweep(&self) {
        self.node_test_cancel.store(true, Ordering::SeqCst);
    }

    fn emit_node_test_progress(&self, progress: NodeTestProgress) {
        let _ = self.app.emit("node-test-progress", progress);
    }

    fn has_connected_node_test_core(&self) -> bool {
        let engine = self.engine.lock();
        engine.status == CoreStatus::On && engine.active.is_some()
    }

    fn node_test_context(&self, node_id: &str) -> Result<(NodeView, ActiveCore, String)> {
        let node = self
            .state
            .lock()
            .nodes
            .iter()
            .find(|node| node.id == node_id && !node.missing)
            .cloned()
            .context("This node is unavailable.")?;
        let definition = self
            .secrets
            .lock()
            .nodes
            .get(node_id)
            .cloned()
            .context("This node definition is unavailable.")?;
        let fingerprint = rules::node_definition_fingerprint(&node, &definition)?;
        let active = {
            let engine = self.engine.lock();
            if engine.status != CoreStatus::On {
                bail!("Connect Clashfork before testing nodes.");
            }
            engine
                .active
                .as_ref()
                .cloned()
                .context("Connect Clashfork before testing nodes.")?
        };
        if active.node_fingerprints.get(node_id) != Some(&fingerprint) {
            bail!("Reconnect Clashfork before testing this updated node.");
        }
        Ok((node, active, fingerprint))
    }

    async fn test_node_inner(self: &Arc<Self>, node_id: String) -> Result<u64> {
        {
            if !self.node_test_in_flight.lock().insert(node_id.clone()) {
                bail!("This node is already being tested.");
            }
        }
        let _in_flight = NodeTestFlight {
            runtime: Arc::clone(self),
            node_id: node_id.clone(),
        };
        let _permit = self
            .node_test_limit
            .acquire()
            .await
            .context("Node tests are unavailable.")?;
        let (node, initial_active, fingerprint) = self.node_test_context(&node_id)?;
        let latency_ms = initial_active
            .core
            .controller
            .test_node_delay(&node.core_name)
            .await
            .map_err(|_| anyhow::anyhow!("No response from this node."))?;

        let _gate = self.apply_gate.lock().await;
        let (current_node, current_active, current_fingerprint) =
            self.node_test_context(&node_id)?;
        if current_active.core.session != initial_active.core.session
            || current_node.core_name != node.core_name
            || current_fingerprint != fingerprint
        {
            bail!("The node or running core changed during the test. Try again.");
        }
        let measured_at_ms = Utc::now().timestamp_millis();
        let mut secrets = self.secrets.lock().clone();
        secrets.node_latencies.insert(
            node_id,
            StoredNodeLatency {
                definition_fingerprint: fingerprint,
                latency_ms,
                measured_at_ms,
            },
        );
        store::save_secrets(&self.paths, &secrets)
            .map_err(|_| anyhow::anyhow!("The node test result could not be saved locally."))?;
        *self.secrets.lock() = secrets;
        drop(_gate);
        self.emit_snapshot();
        Ok(latency_ms)
    }

    async fn enable_vpn(self: &Arc<Self>) -> Result<()> {
        {
            let engine = self.engine.lock();
            if engine.active.is_some() {
                return Ok(());
            }
            if engine.unresolved_owner || engine.unresolved_session.is_some() {
                bail!("A previous owned VPN session needs cleanup before connecting.");
            }
        }
        let startup_started = Instant::now();
        self.refresh_tunnel_advisory();
        let preflight_started = Instant::now();
        let probe = service_client::probe().await;
        let preflight_elapsed = preflight_started.elapsed();
        self.record_startup_stage("service preflight", preflight_elapsed, probe.available);
        {
            let mut engine = self.engine.lock();
            engine.service_available = probe.available;
            engine.service_message.clone_from(&probe.message);
            if !probe.available {
                engine.last_error = probe.message.clone();
                engine.status = CoreStatus::Error;
                drop(engine);
                self.record_startup_total(startup_started.elapsed(), false);
                return Err(anyhow::anyhow!(
                    "{}",
                    probe
                        .message
                        .unwrap_or_else(|| service_client::SERVICE_UNAVAILABLE.to_owned())
                ));
            }
            engine.status = CoreStatus::Starting;
            engine.last_error = None;
        }
        let Some(protocol_info) = probe.protocol_info else {
            let error = anyhow::anyhow!(service_client::SERVICE_NEEDS_REPAIR);
            self.fail_start(&error);
            self.record_startup_total(startup_started.elapsed(), false);
            return Err(error);
        };
        let (state, secrets) = (self.state.lock().clone(), self.secrets.lock().clone());
        if let Err(error) = rules::selection_is_valid(&state, &state.selection) {
            self.fail_start(&error);
            self.record_startup_total(startup_started.elapsed(), false);
            return Err(error);
        }
        let standby = self.engine.lock().standby.clone();
        let controller_config = standby
            .as_ref()
            .map(|core| core.core.controller_config.clone())
            .unwrap_or(ControllerConfig {
                port: pick_port()?,
                secret: random_hex(32),
                dns_port: pick_port()?,
            });
        let compiled = match rules::build_runtime_yaml(
            &state,
            &secrets,
            &state.selection,
            &controller_config,
            true,
        ) {
            Ok(compiled) => compiled,
            Err(error) => {
                self.fail_start(&error);
                self.record_startup_total(startup_started.elapsed(), false);
                return Err(error);
            }
        };
        let validation_started = Instant::now();
        let validation = self.validate_config(&compiled.yaml).await;
        let validation_elapsed = validation_started.elapsed();
        self.record_startup_stage("config validation", validation_elapsed, validation.is_ok());
        if let Err(error) = validation {
            self.fail_start(&error);
            self.record_startup_total(startup_started.elapsed(), false);
            return Err(error);
        }

        if let Some(standby) = standby {
            match service_client::stage(&self.paths, compiled.yaml.clone(), &standby.core.session)
                .await
            {
                Ok(service_client::Staged::Ready(config_path)) => {
                    let active_candidate = ActiveCore {
                        core: standby.core.clone(),
                        yaml: compiled.yaml.clone(),
                        summary: rules::active_summary(&state, &state.selection)
                            .unwrap_or_else(|_| "Connected".into()),
                        mode: state.selection.mode,
                        node_routes: compiled.node_routes.clone(),
                        node_fingerprints: compiled.node_fingerprints.clone(),
                        match_groups: match_groups_by_core(&state),
                    };
                    let reload_started = Instant::now();
                    let reload = standby.core.controller.reload(&config_path).await;
                    self.record_startup_stage(
                        "warm core config reload",
                        reload_started.elapsed(),
                        reload.is_ok(),
                    );
                    let readiness = if let Err(error) = reload {
                        Err(error)
                    } else {
                        service_client::wait_ready_config(
                            &standby.core.controller,
                            match state.selection.mode {
                                Mode::Rule => "rule",
                                Mode::Global => "global",
                            },
                            true,
                        )
                        .await
                        .map(|_| ())
                    };
                    let readiness = match readiness {
                        Ok(()) => {
                            let route_started = Instant::now();
                            let route_result = wait_for_tun_routes_ready(
                                &standby.core.controller,
                                match state.selection.mode {
                                    Mode::Rule => "rule",
                                    Mode::Global => "global",
                                },
                            )
                            .await;
                            self.record_startup_stage(
                                "Windows TUN route handoff",
                                route_started.elapsed(),
                                route_result.is_ok(),
                            );
                            route_result.map(|verified| {
                                if !verified {
                                    self.diagnostics.push(
                                        "app",
                                        "warn",
                                        "Mihomo reports TUN enabled, but Windows route ownership could not be queried; connection readiness is controller-verified only.",
                                    );
                                }
                            })
                        }
                        Err(error) => Err(error),
                    };
                    if let Err(error) = readiness {
                        self.recover_warm_activation_failure(&standby, active_candidate)
                            .await;
                        self.fail_start(&error);
                        self.record_startup_total(startup_started.elapsed(), false);
                        return Err(error);
                    }
                    let _sample_guard = self.sample_gate.write().await;
                    let mut engine = self.engine.lock();
                    engine.standby = None;
                    engine.active = Some(active_candidate);
                    engine.status = CoreStatus::On;
                    engine.last_error = None;
                    engine.generation = self.generation.fetch_add(1, Ordering::SeqCst);
                    engine.pending_reconnect_services.clear();
                    let generation = engine.generation;
                    drop(engine);
                    *self.live.lock() = LiveState {
                        active: true,
                        available: false,
                        generation,
                        ..LiveState::default()
                    };
                    self.diagnostics.push(
                        "app",
                        "info",
                        "VPN enabled through the warm core; TUN routes are ready.",
                    );
                    self.record_startup_total(startup_started.elapsed(), true);
                    return Ok(());
                }
                Ok(service_client::Staged::Restart) => {
                    if let Err(error) =
                        service_client::stop(&self.paths, &standby.core.session).await
                    {
                        self.fail_start(&error);
                        self.record_startup_total(startup_started.elapsed(), false);
                        return Err(error.context(
                            "the warm core could not be stopped before a safe cold start",
                        ));
                    }
                    self.clear_saved_session();
                    self.engine.lock().standby = None;
                    self.diagnostics.push(
                        "app",
                        "warn",
                        "The service declined in-place staging; a confirmed cold start will be used.",
                    );
                }
                Err(error) => {
                    self.fail_start(&error);
                    self.record_startup_total(startup_started.elapsed(), false);
                    return Err(error.context("the warm core could not stage the selected route"));
                }
            }
        }
        let session_token = service_client::new_session_token();
        let pending_proof_save = {
            let mut secrets = self.secrets.lock();
            secrets.last_session = Some(service_client::stored_session(&session_token, 0));
            store::save_secrets(&self.paths, &secrets)
        };
        if let Err(error) = pending_proof_save {
            self.fail_start(&error);
            self.record_startup_total(startup_started.elapsed(), false);
            return Err(
                error.context("the VPN session proof could not be stored before service start")
            );
        }
        let service_start_started = Instant::now();
        let started = match service_client::start(
            &self.paths,
            compiled.yaml.clone(),
            &controller_config,
            session_token.clone(),
            protocol_info,
        )
        .await
        {
            Ok(started) => {
                self.record_startup_stage(
                    "authenticated service start",
                    service_start_started.elapsed(),
                    true,
                );
                started
            }
            Err(error) => {
                self.record_startup_stage(
                    "authenticated service start",
                    service_start_started.elapsed(),
                    false,
                );
                self.reconcile_uncertain_start().await;
                self.fail_start(&error);
                self.record_startup_total(startup_started.elapsed(), false);
                return Err(error);
            }
        };
        let (result, proof, core_path) = started;
        let _ = (result, core_path);
        let save_result = {
            let mut secrets = self.secrets.lock();
            secrets.last_session = Some(service_client::stored_session(
                &session_token,
                proof.generation,
            ));
            store::save_secrets(&self.paths, &secrets)
        };
        if let Err(error) = save_result {
            if service_client::stop(&self.paths, &proof).await.is_ok() {
                self.clear_saved_session();
            } else {
                let mut engine = self.engine.lock();
                engine.unresolved_owner = true;
                engine.unresolved_session = Some(proof.clone());
                engine.last_error = Some(
                    "The VPN session started, but its stop proof could not be saved. Try Disconnect or Quit.".to_owned(),
                );
            }
            self.fail_start(&error);
            self.record_startup_total(startup_started.elapsed(), false);
            return Err(error.context("the VPN session could not be recorded safely"));
        }
        let controller = CoreController::new(
            controller_config.port,
            controller_config.secret.clone(),
            self.http.clone(),
        );
        let core_session = CoreSession {
            session: proof.clone(),
            controller: controller.clone(),
            controller_config: controller_config.clone(),
        };
        let mode_ready_started = Instant::now();
        let ready = service_client::wait_ready_config(
            &controller,
            match state.selection.mode {
                Mode::Rule => "rule",
                Mode::Global => "global",
            },
            true,
        )
        .await;
        if let Err(error) = ready {
            self.record_startup_stage(
                "authenticated controller mode-ready",
                mode_ready_started.elapsed(),
                false,
            );
            if service_client::stop(&self.paths, &proof).await.is_ok() {
                self.clear_saved_session();
            } else {
                self.engine.lock().active = Some(ActiveCore {
                    core: core_session.clone(),
                    yaml: compiled.yaml.clone(),
                    summary: "VPN session needs attention".to_owned(),
                    mode: state.selection.mode,
                    node_routes: compiled.node_routes.clone(),
                    node_fingerprints: compiled.node_fingerprints.clone(),
                    match_groups: match_groups_by_core(&state),
                });
            }
            self.fail_start(&error);
            self.record_startup_total(startup_started.elapsed(), false);
            return Err(error);
        }
        self.record_startup_stage(
            "authenticated controller mode-ready",
            mode_ready_started.elapsed(),
            true,
        );
        let route_started = Instant::now();
        let route_ready = wait_for_tun_routes_ready(
            &core_session.controller,
            match state.selection.mode {
                Mode::Rule => "rule",
                Mode::Global => "global",
            },
        )
        .await;
        self.record_startup_stage(
            "Windows TUN route handoff",
            route_started.elapsed(),
            route_ready.is_ok(),
        );
        match route_ready {
            Ok(false) => self.diagnostics.push(
                "app",
                "warn",
                "Mihomo reports TUN enabled, but Windows route ownership could not be queried; connection readiness is controller-verified only.",
            ),
            Ok(true) => {}
            Err(error) => {
                let active_candidate = ActiveCore {
                    core: core_session,
                    yaml: compiled.yaml,
                    summary: rules::active_summary(&state, &state.selection)
                        .unwrap_or_else(|_| "VPN session needs attention".into()),
                    mode: state.selection.mode,
                    node_routes: compiled.node_routes,
                    node_fingerprints: compiled.node_fingerprints,
                    match_groups: match_groups_by_core(&state),
                };
                if service_client::stop(&self.paths, &active_candidate.core.session)
                    .await
                    .is_ok()
                {
                    self.clear_saved_session();
                } else {
                    self.engine.lock().active = Some(active_candidate);
                }
                self.fail_start(&error);
                self.record_startup_total(startup_started.elapsed(), false);
                return Err(error);
            }
        }
        let summary =
            rules::active_summary(&state, &state.selection).unwrap_or_else(|_| "Connected".into());
        let match_groups = match_groups_by_core(&state);
        let active = ActiveCore {
            core: core_session,
            yaml: compiled.yaml,
            summary: summary.clone(),
            mode: state.selection.mode,
            node_routes: compiled.node_routes,
            node_fingerprints: compiled.node_fingerprints,
            match_groups,
        };
        let _sample_guard = self.sample_gate.write().await;
        let mut engine = self.engine.lock();
        engine.active = Some(active);
        engine.status = CoreStatus::On;
        engine.last_error = None;
        engine.generation = self.generation.fetch_add(1, Ordering::SeqCst);
        engine.pending_reconnect_services.clear();
        let generation = engine.generation;
        drop(engine);
        {
            let mut live = self.live.lock();
            *live = LiveState {
                active: true,
                available: false,
                generation,
                ..LiveState::default()
            };
        }
        self.diagnostics.push(
            "app",
            "info",
            format!("VPN enabled with TUN adapter {}.", rules::TUN_DEVICE),
        );
        self.record_startup_total(startup_started.elapsed(), true);
        let _ = summary;
        Ok(())
    }

    fn record_startup_stage(&self, stage: &str, elapsed: Duration, success: bool) {
        self.diagnostics.push(
            "app",
            "info",
            format!(
                "VPN startup {stage} {} in {} ms.",
                if success { "completed" } else { "failed" },
                elapsed.as_millis()
            ),
        );
    }

    fn record_startup_total(&self, elapsed: Duration, success: bool) {
        self.diagnostics.push(
            "app",
            "info",
            format!(
                "VPN startup {} in {} ms total.",
                if success { "completed" } else { "stopped" },
                elapsed.as_millis()
            ),
        );
    }

    fn fail_start(&self, error: &anyhow::Error) {
        let mut engine = self.engine.lock();
        engine.status = CoreStatus::Error;
        if !engine.unresolved_owner {
            engine.last_error = Some(error.to_string());
        }
        self.diagnostics
            .push("app", "error", format!("VPN start failed: {error}"));
    }

    fn clear_saved_session(&self) {
        let mut secrets = self.secrets.lock();
        secrets.last_session = None;
        if let Err(error) = store::save_secrets(&self.paths, &secrets) {
            self.diagnostics.push(
                "app",
                "warn",
                format!("Could not clear the saved VPN session: {error}"),
            );
        }
    }

    async fn reconcile_uncertain_start(&self) {
        let mut secrets = self.secrets.lock().clone();
        match service_client::already_owned_session(&self.paths, &mut secrets).await {
            Ok(None) => {
                if let Err(error) = store::save_secrets(&self.paths, &secrets) {
                    self.diagnostics.push(
                        "app",
                        "warn",
                        format!("Could not clear the pending VPN stop proof: {error}"),
                    );
                }
                *self.secrets.lock() = secrets;
            }
            Ok(Some(message)) => {
                if let Err(error) = store::save_secrets(&self.paths, &secrets) {
                    self.diagnostics.push(
                        "app",
                        "warn",
                        format!("Could not update the pending VPN stop proof: {error}"),
                    );
                } else {
                    *self.secrets.lock() = secrets.clone();
                }
                let mut engine = self.engine.lock();
                engine.status = CoreStatus::Error;
                engine.last_error = Some(message.clone());
                engine.unresolved_owner = true;
                engine.unresolved_session =
                    secrets
                        .last_session
                        .map(|stored| clashfork_service_ipc::OwnerSessionProof {
                            generation: stored.generation,
                            token: stored.token,
                        });
                drop(engine);
                self.diagnostics.push("service", "error", message);
            }
            Err(error) => {
                let stored = secrets.last_session.clone();
                let mut engine = self.engine.lock();
                engine.status = CoreStatus::Error;
                engine.last_error = Some(format!(
                    "Service cleanup after VPN start could not be confirmed: {error}"
                ));
                engine.unresolved_owner = stored.is_some();
                engine.unresolved_session =
                    stored.map(|stored| clashfork_service_ipc::OwnerSessionProof {
                        generation: stored.generation,
                        token: stored.token,
                    });
                drop(engine);
                self.diagnostics.push(
                    "service",
                    "warn",
                    "Service cleanup after VPN start could not be confirmed.",
                );
            }
        }
    }

    async fn recover_warm_activation_failure(
        &self,
        standby: &StandbyCore,
        uncertain_active: ActiveCore,
    ) {
        let restored = async {
            let staged =
                service_client::stage(&self.paths, standby.yaml.clone(), &standby.core.session)
                    .await?;
            let config_path = match staged {
                service_client::Staged::Ready(path) => path,
                service_client::Staged::Restart => {
                    bail!("the service declined to restore the inert runtime")
                }
            };
            standby.core.controller.reload(&config_path).await?;
            service_client::wait_ready_config(&standby.core.controller, "rule", false).await?;
            if !can_retain_warm_core(true, wait_for_tun_routes_disabled().await) {
                bail!("Windows did not confirm the prior TUN routes were removed")
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if restored.is_ok() {
            let mut engine = self.engine.lock();
            engine.active = None;
            engine.standby = Some(standby.clone());
            engine.status = CoreStatus::Starting;
            return;
        }

        match service_client::stop(&self.paths, &standby.core.session).await {
            Ok(()) => {
                self.clear_saved_session();
                self.paths.cleanup_runtime();
                let mut engine = self.engine.lock();
                engine.active = None;
                engine.standby = None;
                engine.unresolved_session = None;
                engine.unresolved_owner = false;
                engine.status = CoreStatus::Starting;
            }
            Err(error) => {
                let mut engine = self.engine.lock();
                engine.standby = None;
                engine.active = Some(uncertain_active);
                engine.status = CoreStatus::Error;
                engine.last_error = Some(format!(
                    "VPN activation failed and the owned TUN session could not be stopped: {error}"
                ));
                drop(engine);
                self.diagnostics.push(
                    "app",
                    "error",
                    "Warm activation recovery could not confirm that the owned TUN session stopped.",
                );
            }
        }
    }

    async fn disable_vpn(self: &Arc<Self>) -> Result<()> {
        let active = { self.engine.lock().active.clone() };
        let Some(active) = active else {
            let (session, unresolved_owner) = {
                let engine = self.engine.lock();
                (engine.unresolved_session.clone(), engine.unresolved_owner)
            };
            if session.is_some() {
                let mut secrets = self.secrets.lock().clone();
                match service_client::already_owned_session(&self.paths, &mut secrets).await {
                    Ok(None) => {
                        store::save_secrets(&self.paths, &secrets)?;
                        *self.secrets.lock() = secrets;
                        let mut engine = self.engine.lock();
                        engine.unresolved_session = None;
                        engine.unresolved_owner = false;
                        engine.status = CoreStatus::Off;
                        engine.last_error = None;
                        return Ok(());
                    }
                    Ok(Some(message)) => {
                        let _ = store::save_secrets(&self.paths, &secrets);
                        *self.secrets.lock() = secrets.clone();
                        let mut engine = self.engine.lock();
                        engine.unresolved_session = secrets.last_session.map(|stored| {
                            clashfork_service_ipc::OwnerSessionProof {
                                generation: stored.generation,
                                token: stored.token,
                            }
                        });
                        engine.unresolved_owner = true;
                        engine.status = CoreStatus::Error;
                        engine.last_error = Some(message.clone());
                        bail!(message);
                    }
                    Err(error) => {
                        let mut engine = self.engine.lock();
                        engine.status = CoreStatus::Error;
                        engine.last_error = Some(error.to_string());
                        bail!("The previous VPN session could not be stopped safely: {error}");
                    }
                }
            }
            if unresolved_owner {
                bail!(
                    "A previous VPN session is still owned by the service, but its stop proof is unavailable. Reinstall Clashfork Service to clear it safely."
                );
            }
            let mut engine = self.engine.lock();
            engine.status = CoreStatus::Off;
            engine.last_error = None;
            return Ok(());
        };
        let _sample_guard = self.sample_gate.write().await;
        let standby_yaml = rules::build_standby_yaml(&active.core.controller_config)?;
        let disabled = async {
            let staged =
                service_client::stage(&self.paths, standby_yaml.clone(), &active.core.session)
                    .await?;
            let config_path = match staged {
                service_client::Staged::Ready(path) => path,
                service_client::Staged::Restart => {
                    bail!("the service declined in-place TUN shutdown")
                }
            };
            active.core.controller.reload(&config_path).await?;
            service_client::wait_ready_config(&active.core.controller, "rule", false).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let handoff_started = Instant::now();
        let routes_absent = if disabled.is_ok() {
            wait_for_tun_routes_disabled().await
        } else {
            None
        };
        let route_absent = can_retain_warm_core(disabled.is_ok(), routes_absent);
        self.record_startup_stage(
            "Windows TUN route release",
            handoff_started.elapsed(),
            route_absent,
        );

        if route_absent {
            if let Err(error) = active.core.controller.close_connections().await {
                self.diagnostics.push(
                    "app",
                    "warn",
                    format!("Could not close all flows before standby: {error}"),
                );
            }
            {
                let mut engine = self.engine.lock();
                engine.active = None;
                engine.standby = Some(StandbyCore {
                    core: active.core,
                    yaml: standby_yaml,
                });
                engine.status = CoreStatus::Off;
                engine.last_error = None;
                engine.generation = self.generation.fetch_add(1, Ordering::SeqCst);
                engine.pending_reconnect_services.clear();
            }
            *self.live.lock() = LiveState::default();
            self.diagnostics.push(
                "app",
                "info",
                "VPN disabled; the owned core remains warm with TUN and traffic listeners disabled.",
            );
            return Ok(());
        }

        self.diagnostics.push(
            "app",
            "warn",
            "In-place TUN shutdown was not confirmed; stopping the owned core instead.",
        );
        match service_client::stop(&self.paths, &active.core.session).await {
            Ok(()) => {
                self.clear_saved_session();
                self.paths.cleanup_runtime();
                {
                    let mut engine = self.engine.lock();
                    engine.active = None;
                    engine.standby = None;
                    engine.unresolved_session = None;
                    engine.unresolved_owner = false;
                    engine.status = CoreStatus::Off;
                    engine.last_error = None;
                    engine.generation = self.generation.fetch_add(1, Ordering::SeqCst);
                    engine.pending_reconnect_services.clear();
                }
                *self.live.lock() = LiveState::default();
                self.diagnostics.push(
                    "app",
                    "info",
                    "VPN disabled; the owned core and TUN adapter were stopped.",
                );
                Ok(())
            }
            Err(error) => {
                let mut engine = self.engine.lock();
                engine.status = CoreStatus::Error;
                engine.last_error = Some(error.to_string());
                drop(engine);
                self.diagnostics
                    .push("app", "error", format!("VPN stop failed: {error}"));
                bail!("The VPN could not be stopped safely: {error}");
            }
        }
    }

    pub async fn shutdown(self: &Arc<Self>) {
        self.quit_requested.store(true, Ordering::SeqCst);
        let _guard = self.apply_gate.lock().await;
        let _sample_guard = self.sample_gate.write().await;
        let (active, standby, unresolved_session, unresolved_owner) = {
            let engine = self.engine.lock();
            (
                engine.active.clone(),
                engine.standby.clone(),
                engine.unresolved_session.clone(),
                engine.unresolved_owner,
            )
        };
        let session = active
            .as_ref()
            .map(|core| core.core.session.clone())
            .or_else(|| standby.as_ref().map(|core| core.core.session.clone()));
        let stopped = if let Some(session) = session {
            match service_client::stop(&self.paths, &session).await {
                Ok(()) => true,
                Err(error) => {
                    self.diagnostics.push(
                        "app",
                        "warn",
                        format!("Owned core cleanup at exit failed: {error}"),
                    );
                    false
                }
            }
        } else if unresolved_session.is_some() || unresolved_owner {
            let mut secrets = self.secrets.lock().clone();
            match service_client::already_owned_session(&self.paths, &mut secrets).await {
                Ok(None) => {
                    if let Err(error) = store::save_secrets(&self.paths, &secrets) {
                        self.diagnostics.push(
                            "app",
                            "warn",
                            format!("Could not clear the recovered stop proof: {error}"),
                        );
                        false
                    } else {
                        *self.secrets.lock() = secrets;
                        true
                    }
                }
                Ok(Some(_)) => false,
                Err(error) => {
                    self.diagnostics.push(
                        "app",
                        "warn",
                        format!("Previous VPN session cleanup at exit failed: {error}"),
                    );
                    false
                }
            }
        } else {
            self.secrets.lock().last_session.is_none()
        };
        if stopped {
            self.clear_saved_session();
            let mut engine = self.engine.lock();
            engine.active = None;
            engine.standby = None;
            engine.unresolved_session = None;
            engine.unresolved_owner = false;
            engine.status = CoreStatus::Off;
        }
        self.paths.cleanup_runtime();
    }

    pub async fn commit_mutation(
        self: &Arc<Self>,
        candidate: AppState,
        apply_runtime: bool,
    ) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        self.commit_locked(candidate, apply_runtime).await?;
        self.emit_snapshot();
        crate::tray::refresh(self)?;
        Ok(self.snapshot())
    }

    async fn commit_locked(
        self: &Arc<Self>,
        mut candidate: AppState,
        apply_runtime: bool,
    ) -> Result<()> {
        recompute_counts(&mut candidate);
        let previous = self.state.lock().clone();
        let active = { self.engine.lock().active.is_some() };
        if apply_runtime && active {
            rules::selection_is_valid(&candidate, &candidate.selection)?;
            store::save_state(&self.paths, &candidate)?;
            if let Err(error) = self.apply_candidate_config(&candidate).await {
                if let Err(restore) = store::save_state(&self.paths, &previous) {
                    self.diagnostics.push(
                        "app",
                        "error",
                        format!("Could not restore prior state after a rejected routing switch: {restore}"),
                    );
                }
                self.engine.lock().last_error = Some(error.to_string());
                self.emit_snapshot();
                return Err(error);
            }
            self.engine.lock().last_error = None;
        } else if let Err(error) = store::save_state(&self.paths, &candidate) {
            return Err(error);
        }
        *self.state.lock() = candidate;
        Ok(())
    }

    async fn apply_candidate_config(self: &Arc<Self>, candidate: &AppState) -> Result<()> {
        let active = {
            let engine = self.engine.lock();
            engine
                .active
                .as_ref()
                .cloned()
                .context("The VPN is not connected; routing cannot be applied.")?
        };
        let secrets = self.secrets.lock().clone();
        let compiled = rules::build_runtime_yaml(
            candidate,
            &secrets,
            &candidate.selection,
            &active.core.controller_config,
            true,
        )?;
        self.validate_config(&compiled.yaml).await?;
        let staged =
            service_client::stage(&self.paths, compiled.yaml.clone(), &active.core.session).await?;
        let config_path = match staged {
            service_client::Staged::Ready(path) => path,
            service_client::Staged::Restart => {
                bail!(
                    "The Clashfork service requires a full restart to apply this routing change. The current routing was kept."
                );
            }
        };
        let _sample_guard = self.sample_gate.write().await;
        if let Err(error) = active.core.controller.reload(&config_path).await {
            return Err(error.context(self.rollback_runtime(&active).await));
        }
        if let Err(error) = self
            .verify_mode(&active.core.controller, candidate.selection.mode)
            .await
        {
            return Err(error.context(self.rollback_runtime(&active).await));
        }
        if let Err(error) = active.core.controller.close_connections().await {
            self.diagnostics.push(
                "app",
                "warn",
                format!("Could not close all existing flows after routing switch: {error}"),
            );
        }
        let summary = rules::active_summary(candidate, &candidate.selection)?;
        let match_groups = match_groups_by_core(candidate);
        let generation = {
            let mut engine = self.engine.lock();
            if let Some(active_state) = engine.active.as_mut() {
                active_state.yaml = compiled.yaml;
                active_state.summary = summary;
                active_state.mode = candidate.selection.mode;
                active_state.node_routes = compiled.node_routes;
                active_state.node_fingerprints = compiled.node_fingerprints;
                active_state.match_groups = match_groups;
            }
            engine.generation = self.generation.fetch_add(1, Ordering::SeqCst);
            engine.pending_reconnect_services.clear();
            engine.generation
        };
        {
            let mut live = self.live.lock();
            live.active = true;
            live.available = false;
            live.download_rate = 0;
            live.upload_rate = 0;
            live.proxy_download_rate = 0;
            live.proxy_upload_rate = 0;
            live.connections.clear();
            live.flow_counters.clear();
            live.last_sample_at = None;
            live.generation = generation;
        }
        self.diagnostics.push(
            "app",
            "info",
            "Routing switched and existing flows were closed.",
        );
        Ok(())
    }

    async fn rollback_runtime(&self, active: &ActiveCore) -> &'static str {
        if let Ok(service_client::Staged::Ready(path)) =
            service_client::stage(&self.paths, active.yaml.clone(), &active.core.session).await
            && active.core.controller.reload(&path).await.is_ok()
            && self
                .verify_mode(&active.core.controller, active.mode)
                .await
                .is_ok()
        {
            self.diagnostics.push(
                "app",
                "warn",
                "Routing change failed; previous routing restored.",
            );
            return "The routing change failed. Previous routing was restored.";
        }
        if let Err(error) = service_client::stop(&self.paths, &active.core.session).await {
            let message = "The routing change failed and recovery could not stop the TUN session. Try Disconnect.";
            {
                let mut engine = self.engine.lock();
                engine.status = CoreStatus::Error;
                engine.last_error = Some(message.to_owned());
            }
            self.diagnostics
                .push("app", "error", format!("TUN recovery failed: {error}"));
            self.emit_snapshot();
            return message;
        }
        let message = "The routing change failed and the previous policy could not be restored. The TUN session was stopped.";
        {
            let mut engine = self.engine.lock();
            engine.active = None;
            engine.standby = None;
            engine.unresolved_session = None;
            engine.unresolved_owner = false;
            engine.status = CoreStatus::Off;
            engine.last_error = Some(message.to_owned());
        }
        {
            let mut secrets = self.secrets.lock();
            secrets.last_session = None;
            let _ = store::save_secrets(&self.paths, &secrets);
        }
        {
            let mut live = self.live.lock();
            *live = LiveState::default();
        }
        self.paths.cleanup_runtime();
        self.emit_snapshot();
        message
    }

    async fn verify_mode(&self, controller: &CoreController, mode: Mode) -> Result<()> {
        let actual = controller.config_mode().await?;
        let expected = match mode {
            Mode::Rule => "rule",
            Mode::Global => "global",
        };
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("Mihomo did not enter {expected} mode.");
        }
        Ok(())
    }

    async fn validate_config(&self, yaml: &str) -> Result<()> {
        let core = self.paths.bundled_core().context(
            "The bundled Mihomo core is missing; reinstall Clashfork before connecting.",
        )?;
        let check_dir = self.paths.runtime_dir.join("core-check");
        if check_dir.exists() {
            let _ = std::fs::remove_dir_all(&check_dir);
        }
        std::fs::create_dir_all(&check_dir)?;
        let config_path = self
            .paths
            .ephemeral_config(&format!("check-{}.yaml", Uuid::new_v4()));
        security::write_private_file(&config_path, yaml.as_bytes())?;
        // Mihomo is a console-subsystem binary. Its preflight runs in the
        // background immediately before Connect, so suppress a child console
        // while retaining stdout/stderr capture for validation diagnostics.
        let mut command = tokio::process::Command::new(&core);
        command
            .arg("-t")
            .arg("-d")
            .arg(&check_dir)
            .arg("-f")
            .arg(&config_path)
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let result = tokio::time::timeout(Duration::from_secs(25), command.output()).await;
        let _ = std::fs::remove_file(&config_path);
        let _ = std::fs::remove_dir_all(&check_dir);
        let output = result
            .map_err(|_| anyhow::anyhow!("Stock Mihomo configuration validation timed out."))?
            .context("Could not run the bundled stock Mihomo core.")?;
        if !output.status.success() {
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            self.diagnostics.push(
                "core",
                "error",
                format!(
                    "Stock Mihomo rejected the generated routing configuration:\n{}",
                    crate::diagnostics::redact(&combined)
                ),
            );
            bail!(
                "Stock Mihomo rejected the generated routing configuration. Review the selected nodes and conditions."
            );
        }
        Ok(())
    }

    pub async fn apply_selection(
        self: &Arc<Self>,
        mode: Mode,
        ruleset_id: Option<String>,
        global_node_id: Option<String>,
    ) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        candidate.selection = crate::models::Selection {
            mode,
            ruleset_id,
            global_node_id,
        };
        rules::selection_is_valid(&candidate, &candidate.selection)?;
        let active = { self.engine.lock().active.is_some() };
        self.commit_locked(candidate, active).await?;
        self.emit_snapshot();
        crate::tray::refresh(self)?;
        Ok(self.snapshot())
    }

    pub async fn save_preset(self: &Arc<Self>, name: String) -> Result<Snapshot> {
        let name = rules::validate_group_name(&name).context("Invalid preset name")?;
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        if candidate
            .presets
            .iter()
            .any(|preset| preset.name.eq_ignore_ascii_case(&name))
        {
            bail!("A saved run configuration with this name already exists.");
        }
        rules::selection_is_valid(&candidate, &candidate.selection)?;
        candidate.presets.push(crate::models::Preset {
            id: rules::generate_id("preset"),
            name,
            mode: candidate.selection.mode,
            ruleset_id: candidate.selection.ruleset_id.clone(),
            global_node_id: candidate.selection.global_node_id.clone(),
        });
        store::save_state(&self.paths, &candidate)?;
        *self.state.lock() = candidate;
        self.emit_snapshot();
        crate::tray::refresh(self)?;
        Ok(self.snapshot())
    }

    pub async fn delete_preset(self: &Arc<Self>, preset_id: &str) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        candidate.presets.retain(|preset| preset.id != preset_id);
        store::save_state(&self.paths, &candidate)?;
        *self.state.lock() = candidate;
        self.emit_snapshot();
        crate::tray::refresh(self)?;
        Ok(self.snapshot())
    }

    pub async fn apply_preset(self: &Arc<Self>, preset_id: &str) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        let preset = candidate
            .presets
            .iter()
            .find(|preset| preset.id == preset_id)
            .cloned()
            .context("The saved run configuration no longer exists.")?;
        candidate.selection = crate::models::Selection {
            mode: preset.mode,
            ruleset_id: preset.ruleset_id,
            global_node_id: preset.global_node_id,
        };
        rules::selection_is_valid(&candidate, &candidate.selection)?;
        let active = { self.engine.lock().active.is_some() };
        self.commit_locked(candidate, active).await?;
        self.emit_snapshot();
        crate::tray::refresh(self)?;
        Ok(self.snapshot())
    }

    pub async fn save_match_group(
        self: &Arc<Self>,
        input: crate::models::SaveMatchGroupInput,
    ) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        let name = rules::validate_group_name(&input.name).context("Invalid match group name")?;
        if candidate.match_groups.iter().any(|group| {
            group.name.eq_ignore_ascii_case(&name) && Some(&group.id) != input.id.as_ref()
        }) {
            bail!("A match group with this name already exists.");
        }
        let id = input.id.unwrap_or_else(|| rules::generate_id("group"));
        let mut conditions = Vec::new();
        for draft in input.conditions {
            let value = rules::validate_condition_value(draft.kind, &draft.value)?;
            conditions.push(crate::models::MatchCondition {
                id: rules::generate_id("condition"),
                kind: draft.kind,
                value,
            });
        }
        let group = crate::models::MatchGroup {
            id: id.clone(),
            name,
            conditions,
        };
        rules::validate_match_group(&group)?;
        if let Some(existing) = candidate
            .match_groups
            .iter_mut()
            .find(|group| group.id == id)
        {
            *existing = group;
        } else {
            candidate.match_groups.push(group);
        }
        let active_uses = active_ruleset_uses_group(&candidate, &id);
        let apply = { self.engine.lock().active.is_some() } && active_uses;
        self.commit_locked(candidate, apply).await?;
        self.emit_snapshot();
        Ok(self.snapshot())
    }

    pub async fn delete_match_group(self: &Arc<Self>, group_id: &str) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        if candidate.rulesets.iter().any(|ruleset| {
            ruleset
                .entries
                .iter()
                .any(|entry| entry.match_group_id == group_id)
        }) {
            bail!("This match group is used by a ruleset. Remove it from the ruleset first.");
        }
        candidate.match_groups.retain(|group| group.id != group_id);
        store::save_state(&self.paths, &candidate)?;
        *self.state.lock() = candidate;
        self.emit_snapshot();
        Ok(self.snapshot())
    }

    pub async fn save_ruleset(
        self: &Arc<Self>,
        input: crate::models::SaveRulesetInput,
    ) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        let name = rules::validate_ruleset_name(&input.name)?;
        if candidate.rulesets.iter().any(|ruleset| {
            ruleset.name.eq_ignore_ascii_case(&name) && Some(&ruleset.id) != input.id.as_ref()
        }) {
            bail!("A ruleset with this name already exists.");
        }
        let id = input
            .id
            .clone()
            .unwrap_or_else(|| rules::generate_id("ruleset"));
        let mut entries = Vec::new();
        for mut entry in input.entries {
            if entry.id.trim().is_empty() {
                entry.id = rules::generate_id("entry");
            }
            if entry.conditions.is_empty() {
                let group = candidate
                    .match_groups
                    .iter()
                    .find(|group| group.id == entry.match_group_id)
                    .context("A rule needs at least one condition.")?;
                entry.name = group.name.clone();
                entry.conditions = group.conditions.clone();
            }
            if let crate::models::Destination::Node { node_id } = &entry.destination {
                rules::validate_node_reference(&candidate, node_id)?;
            }
            entries.push(entry);
        }
        if let crate::models::Miscellaneous::Node { node_id } = &input.miscellaneous {
            rules::validate_node_reference(&candidate, node_id)?;
        }
        let ruleset = crate::models::Ruleset {
            id: id.clone(),
            name,
            entries,
            miscellaneous: input.miscellaneous,
        };
        rules::validate_ruleset(&ruleset, &candidate)?;
        if let Some(existing) = candidate
            .rulesets
            .iter_mut()
            .find(|ruleset| ruleset.id == id)
        {
            *existing = ruleset;
        } else {
            candidate.rulesets.push(ruleset);
            if candidate.selection.ruleset_id.is_none() {
                candidate.selection.ruleset_id = Some(id.clone());
            }
        }
        let apply = { self.engine.lock().active.is_some() }
            && candidate.selection.mode == Mode::Rule
            && candidate.selection.ruleset_id.as_deref() == Some(&id);
        self.commit_locked(candidate, apply).await?;
        self.emit_snapshot();
        if let Err(error) = crate::tray::refresh(self) {
            self.diagnostics.push(
                "app",
                "warn",
                format!("The ruleset was saved, but the tray menu could not be refreshed: {error}"),
            );
        }
        Ok(self.snapshot())
    }

    pub async fn delete_ruleset(self: &Arc<Self>, ruleset_id: &str) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        if candidate.selection.ruleset_id.as_deref() == Some(ruleset_id) {
            bail!("The active ruleset cannot be deleted. Select another ruleset first.");
        }
        if candidate
            .presets
            .iter()
            .any(|preset| preset.ruleset_id.as_deref() == Some(ruleset_id))
        {
            bail!("A saved run configuration uses this ruleset. Delete that preset first.");
        }
        candidate
            .rulesets
            .retain(|ruleset| ruleset.id != ruleset_id);
        store::save_state(&self.paths, &candidate)?;
        *self.state.lock() = candidate;
        self.emit_snapshot();
        Ok(self.snapshot())
    }

    pub async fn rename_service(
        self: &Arc<Self>,
        service_id: &str,
        title: &str,
    ) -> Result<Snapshot> {
        let title = sanitize_service_title(title)?;
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        let service = candidate
            .services
            .iter_mut()
            .find(|service| service.id == service_id)
            .context("The service no longer exists.")?;
        service.title = title.clone();
        for node in candidate
            .nodes
            .iter_mut()
            .filter(|node| node.service_id == service_id)
        {
            node.service_title = title.clone();
        }
        if let Some(active) = self.engine.lock().active.as_mut() {
            for (core_name, label) in active.node_routes.iter_mut() {
                if let Some(node) = candidate
                    .nodes
                    .iter()
                    .find(|node| &node.core_name == core_name)
                {
                    *label = format!("{} / {}", node.service_title, node.display_name);
                }
            }
            if let Ok(summary) = rules::active_summary(&candidate, &candidate.selection) {
                active.summary = summary;
            }
        }
        store::save_state(&self.paths, &candidate)?;
        *self.state.lock() = candidate;
        self.emit_snapshot();
        crate::tray::refresh(self)?;
        Ok(self.snapshot())
    }

    pub async fn preview_import(self: &Arc<Self>, input: String) -> Result<ImportPreview> {
        let source = crate::subscription::fetch_source(&self.subscription_http, &input).await?;
        let id = rules::generate_id("preview");
        let source_type = crate::subscription::source_type_for_input(&input)?;
        let mut preview = source.preview(id.clone(), source_type);
        if source_type == SourceType::Link {
            preview.title = "Manual nodes".to_owned();
        }
        preview.nodes.truncate(500);
        let pending = PendingImport {
            id: id.clone(),
            title: preview.title.clone(),
            source_type,
            source_url: if source_type == SourceType::Subscription {
                Some(input)
            } else {
                None
            },
            source_host: preview.source_host.clone(),
            nodes: source.nodes,
            unsupported_count: source.unsupported_count,
            notice_count: source.notice_count,
            allowance: source.allowance,
        };
        self.previews.lock().insert(id, pending);
        Ok(preview)
    }

    pub fn cancel_import(&self, preview_id: &str) {
        self.previews.lock().remove(preview_id);
    }

    pub async fn confirm_import(
        self: &Arc<Self>,
        preview_id: &str,
        title_override: Option<String>,
    ) -> Result<Snapshot> {
        let _gate = self.apply_gate.lock().await;
        let pending = self
            .previews
            .lock()
            .remove(preview_id)
            .context("This import preview expired. Preview the source again.")?;
        let mut candidate = self.state.lock().clone();
        let mut secrets = self.secrets.lock().clone();
        let title = if let Some(title) = title_override {
            sanitize_service_title(&title)?
        } else {
            sanitize_service_title(&pending.title)?
        };
        let service_id;
        if pending.source_type == SourceType::Link {
            let manual_id = candidate
                .services
                .iter()
                .find(|service| service.source_type == SourceType::Manual)
                .map(|service| service.id.clone());
            if let Some(id) = manual_id {
                service_id = id;
            } else {
                service_id = rules::generate_id("service");
                candidate.services.push(ServiceView {
                    id: service_id.clone(),
                    title: "Manual nodes".to_owned(),
                    source_type: SourceType::Manual,
                    source_host: None,
                    node_count: 0,
                    missing_count: 0,
                    allowance: crate::models::Allowance::Unavailable,
                    stale: false,
                    omitted_node_ids: Vec::new(),
                    last_refresh_at: Some(Utc::now().timestamp()),
                    last_error: None,
                });
            }
        } else {
            service_id = rules::generate_id("service");
            if let Some(url) = &pending.source_url {
                secrets
                    .subscriptions
                    .insert(service_id.clone(), url.clone());
            }
            candidate.services.push(ServiceView {
                id: service_id.clone(),
                title,
                source_type: SourceType::Subscription,
                source_host: pending.source_host.clone(),
                node_count: 0,
                missing_count: 0,
                allowance: pending.allowance.clone(),
                stale: false,
                omitted_node_ids: Vec::new(),
                last_refresh_at: Some(Utc::now().timestamp()),
                last_error: None,
            });
        }
        let service_title = candidate
            .services
            .iter()
            .find(|service| service.id == service_id)
            .map(|service| service.title.clone())
            .unwrap_or_else(|| "Manual nodes".to_owned());
        crate::subscription::prepend_nodes(
            &mut candidate,
            &mut secrets,
            &service_id,
            &service_title,
            pending.nodes,
        );
        recompute_counts(&mut candidate);
        store::save_secrets(&self.paths, &secrets)?;
        if let Err(error) = store::save_state(&self.paths, &candidate) {
            return Err(error);
        }
        *self.state.lock() = candidate;
        *self.secrets.lock() = secrets;
        self.emit_snapshot();
        crate::tray::refresh(self)?;
        Ok(self.snapshot())
    }

    pub async fn refresh_service(self: &Arc<Self>, service_id: &str) -> Result<Snapshot> {
        let flight = {
            let mut flights = self.refresh_flights.lock();
            flights.retain(|_, flight| flight.strong_count() > 0);
            if let Some(flight) = flights.get(service_id).and_then(Weak::upgrade) {
                flight
            } else {
                let flight = Arc::new(tokio::sync::Mutex::new(()));
                flights.insert(service_id.to_owned(), Arc::downgrade(&flight));
                flight
            }
        };
        let _flight = flight.lock().await;
        let (service, url) = {
            let state = self.state.lock();
            let service = state
                .services
                .iter()
                .find(|service| service.id == service_id)
                .cloned()
                .context("The service no longer exists.")?;
            let url = self.secrets.lock().subscriptions.get(service_id).cloned();
            (service, url)
        };
        if service.source_type == SourceType::Manual {
            bail!("Manual nodes cannot be refreshed.");
        }
        let url = url.context("The subscription URL is unavailable; re-add the service.")?;
        let fetched = crate::subscription::fetch_source(&self.subscription_http, &url).await;

        // Network I/O deliberately happens before apply_gate. Rebase the result
        // on the newest state, and discard it if the source changed in flight.
        let _gate = self.apply_gate.lock().await;
        let mut candidate = self.state.lock().clone();
        let mut secrets = self.secrets.lock().clone();
        let current_service = candidate
            .services
            .iter()
            .find(|service| service.id == service_id)
            .context("The service was removed while its refresh was in progress.")?;
        let current_url = secrets.subscriptions.get(service_id);
        if current_url != Some(&url) {
            bail!("The service source changed while refresh was in progress. Refresh it again.");
        }
        let current_title = current_service.title.clone();
        match fetched {
            Ok(source) => {
                let old_nodes: Vec<NodeView> = candidate
                    .nodes
                    .iter()
                    .filter(|node| node.service_id == service_id)
                    .cloned()
                    .collect();
                let old_defs: HashMap<String, serde_yaml_ng::Value> = old_nodes
                    .iter()
                    .filter_map(|node| {
                        secrets
                            .nodes
                            .get(&node.id)
                            .cloned()
                            .map(|value| (node.id.clone(), value))
                    })
                    .collect();
                let referenced = referenced_node_ids(&candidate);
                let active_refs = active_node_references(&candidate);
                let pending_omissions: HashSet<String> =
                    current_service.omitted_node_ids.iter().cloned().collect();
                let sources = source.nodes;
                let reconciled_ids = reconcile_node_ids(service_id, &old_nodes, &sources);
                let mut new_ids = HashSet::new();
                let mut new_public = Vec::new();
                let mut new_secrets = Vec::new();
                let mut changed_active_definition = false;
                for (node, id) in sources.into_iter().zip(reconciled_ids) {
                    let definition_changed =
                        old_defs.get(&id).is_some_and(|old| old != &node.proxy);
                    if active_refs.contains(&id) && definition_changed {
                        changed_active_definition = true;
                    }
                    if definition_changed {
                        secrets.node_latencies.remove(&id);
                    }
                    new_ids.insert(id.clone());
                    new_public.push(NodeView {
                        id: id.clone(),
                        service_id: service_id.to_owned(),
                        service_title: current_title.clone(),
                        display_name: node.display_name,
                        protocol: node.protocol,
                        missing: false,
                        omitted_once: false,
                        core_name: crate::subscription::node_core_name(&id),
                        latency_ms: None,
                        latency_measured_at_ms: None,
                    });
                    new_secrets.push((id, node.proxy));
                }
                candidate.nodes.retain(|node| node.service_id != service_id);
                secrets
                    .nodes
                    .retain(|id, _| !old_nodes.iter().any(|node| &node.id == id));
                for (id, proxy) in new_secrets {
                    secrets.nodes.insert(id, proxy);
                }
                candidate.nodes.extend(new_public);
                let mut new_omitted_ids = Vec::new();
                for old in old_nodes {
                    if referenced.contains(&old.id) && !new_ids.contains(&old.id) {
                        let mut missing = old;
                        let confirmed_missing = omission_is_confirmed(&missing, &pending_omissions);
                        missing.missing = confirmed_missing;
                        missing.omitted_once = !confirmed_missing;
                        if !confirmed_missing {
                            new_omitted_ids.push(missing.id.clone());
                        }
                        if let Some(proxy) = old_defs.get(&missing.id) {
                            secrets.nodes.insert(missing.id.clone(), proxy.clone());
                        }
                        candidate.nodes.push(missing);
                    }
                }
                let active_note = missing_active_note(&candidate, &service_id);
                let retained_node_ids: HashSet<&str> = candidate
                    .nodes
                    .iter()
                    .map(|node| node.id.as_str())
                    .collect();
                secrets
                    .node_latencies
                    .retain(|id, _| retained_node_ids.contains(id.as_str()));
                let update_notice = changed_active_definition;
                if let Some(service) = candidate
                    .services
                    .iter_mut()
                    .find(|service| service.id == service_id)
                {
                    service.allowance = source.allowance;
                    service.stale = false;
                    service.omitted_node_ids = new_omitted_ids;
                    service.last_refresh_at = Some(Utc::now().timestamp());
                    service.last_error = active_note;
                    service.source_host = source.source_host;
                }
                if update_notice && self.engine.lock().active.is_some() {
                    self.engine
                        .lock()
                        .pending_reconnect_services
                        .insert(service_id.to_owned());
                    self.diagnostics.push(
                        "app",
                        "info",
                        "A selected node was updated by refresh. The connected session keeps its current endpoint until the next reconnect.",
                    );
                }
                recompute_counts(&mut candidate);
                store::save_secrets(&self.paths, &secrets)?;
                store::save_state(&self.paths, &candidate)?;
                *self.state.lock() = candidate;
                *self.secrets.lock() = secrets;
                self.diagnostics
                    .push("app", "info", "Subscription nodes refreshed.");
            }
            Err(error) => {
                let message = mark_refresh_failed(&mut candidate, service_id, &error);
                store::save_state(&self.paths, &candidate)?;
                *self.state.lock() = candidate;
                if let Some(message) = message {
                    self.diagnostics.push("app", "warn", message);
                } else {
                    self.diagnostics.push(
                        "app",
                        "debug",
                        "Subscription refresh did not complete; saved nodes and allowance remain available.",
                    );
                }
            }
        }
        self.emit_snapshot();
        crate::tray::refresh(self)?;
        Ok(self.snapshot())
    }

    pub fn set_history_paused(self: &Arc<Self>, paused: bool) -> Result<Snapshot> {
        let mut candidate = self.state.lock().clone();
        candidate.history_paused = paused;
        store::save_state(&self.paths, &candidate)?;
        *self.state.lock() = candidate;
        self.emit_snapshot();
        Ok(self.snapshot())
    }

    pub fn clear_history(self: &Arc<Self>) -> Result<Snapshot> {
        let mut history = self.history.lock();
        history.clear()?;
        drop(history);
        let mut live = self.live.lock();
        live.flow_counters.clear();
        self.diagnostics
            .push("app", "info", "Traffic history cleared.");
        self.emit_snapshot();
        Ok(self.snapshot())
    }

    pub fn save_custom_group(self: &Arc<Self>, group: CustomAppGroup) -> Result<Snapshot> {
        let name = sanitize_group_name(&group.name)?;
        let mut candidate = self.state.lock().clone();
        if candidate
            .custom_app_groups
            .iter()
            .any(|existing| existing.name.eq_ignore_ascii_case(&name) && existing.id != group.id)
        {
            bail!("A custom app group with this name already exists.");
        }
        let mut apps = Vec::new();
        let mut seen = HashSet::new();
        for app in group.apps {
            let app = app.trim();
            if app.is_empty() {
                continue;
            }
            if app.chars().any(|c| c.is_control() || c == ',') || app.len() > 120 {
                bail!("App names must be simple names without commas or control characters.");
            }
            if seen.insert(app.to_ascii_lowercase()) {
                apps.push(app.to_owned());
            }
        }
        if apps.is_empty() {
            bail!("Add at least one app name to the group.");
        }
        let group = CustomAppGroup {
            id: if group.id.trim().is_empty() {
                rules::generate_id("appgroup")
            } else {
                group.id
            },
            name,
            apps,
        };
        if let Some(existing) = candidate
            .custom_app_groups
            .iter_mut()
            .find(|existing| existing.id == group.id)
        {
            *existing = group;
        } else {
            candidate.custom_app_groups.push(group);
        }
        store::save_state(&self.paths, &candidate)?;
        *self.state.lock() = candidate;
        self.emit_snapshot();
        Ok(self.snapshot())
    }

    pub fn delete_custom_group(self: &Arc<Self>, group_id: &str) -> Result<Snapshot> {
        let mut candidate = self.state.lock().clone();
        candidate
            .custom_app_groups
            .retain(|group| group.id != group_id);
        store::save_state(&self.paths, &candidate)?;
        *self.state.lock() = candidate;
        self.emit_snapshot();
        Ok(self.snapshot())
    }

    pub fn history_report(&self, input: HistoryInput) -> Result<HistoryReport> {
        let groups = self.state.lock().custom_app_groups.clone();
        let history = self.history.lock();
        history.report_with_filters(
            &input.range,
            &input.breakdown,
            &groups,
            Utc::now().timestamp(),
            input.route_filter,
            input.subject_filter.as_ref(),
        )
    }

    pub fn live_snapshot(&self) -> LiveSnapshot {
        let live = self.live.lock();
        LiveSnapshot {
            active: live.active,
            available: live.available,
            download_rate: live.download_rate,
            upload_rate: live.upload_rate,
            download_total: live.download_total,
            upload_total: live.upload_total,
            connections: live.connections.clone(),
            sampled_at: live.sampled_at,
        }
    }

    pub fn live_rates(&self) -> LiveRates {
        let live = self.live.lock();
        LiveRates {
            active: live.active,
            available: live.available,
            proxy_download_rate: live.proxy_download_rate,
            proxy_upload_rate: live.proxy_upload_rate,
            sampled_at: live.sampled_at,
        }
    }

    pub fn diagnostics_snapshot(&self) -> Vec<crate::models::DiagnosticEntry> {
        self.diagnostics.snapshot()
    }

    pub fn current_active(&self) -> Option<ActiveCore> {
        self.engine.lock().active.clone()
    }

    pub fn current_selection(&self) -> crate::models::Selection {
        self.state.lock().selection.clone()
    }
}

async fn startup_service_probe(runtime: Arc<AppRuntime>) {
    let _gate = runtime.apply_gate.lock().await;
    runtime.refresh_tunnel_advisory();
    let probe = service_client::probe().await;
    {
        let mut engine = runtime.engine.lock();
        engine.service_available = probe.available;
        engine.service_message = probe.message.clone();
    }
    let mut prime = false;
    if probe.available {
        let mut secrets = runtime.secrets.lock().clone();
        match service_client::already_owned_session(&runtime.paths, &mut secrets).await {
            Ok(Some(message)) => {
                if let Err(error) = store::save_secrets(&runtime.paths, &secrets) {
                    runtime.diagnostics.push(
                        "app",
                        "warn",
                        format!("Could not update the previous session proof: {error}"),
                    );
                } else {
                    *runtime.secrets.lock() = secrets.clone();
                }
                let stored = secrets.last_session.clone();
                let mut engine = runtime.engine.lock();
                engine.status = CoreStatus::Error;
                engine.last_error = Some(message.clone());
                engine.unresolved_owner = true;
                engine.unresolved_session =
                    stored.map(|session| clashfork_service_ipc::OwnerSessionProof {
                        generation: session.generation,
                        token: session.token,
                    });
                drop(engine);
                runtime.diagnostics.push("service", "error", message);
            }
            Ok(None) => {
                if let Err(error) = store::save_secrets(&runtime.paths, &secrets) {
                    runtime.diagnostics.push(
                        "app",
                        "warn",
                        format!("Could not clear the stale VPN session: {error}"),
                    );
                } else {
                    *runtime.secrets.lock() = secrets;
                    prime = true;
                }
            }
            Err(error) => {
                let message = format!("Previous session cleanup could not run: {error}");
                let stored = secrets.last_session.clone();
                let mut engine = runtime.engine.lock();
                engine.status = CoreStatus::Error;
                engine.last_error = Some(message.clone());
                engine.unresolved_owner = stored.is_some();
                engine.unresolved_session =
                    stored.map(|session| clashfork_service_ipc::OwnerSessionProof {
                        generation: session.generation,
                        token: session.token,
                    });
                drop(engine);
                runtime.diagnostics.push("service", "warn", message);
            }
        }
    } else if let Some(stored) = runtime.secrets.lock().last_session.clone() {
        let message = probe
            .message
            .clone()
            .unwrap_or_else(|| service_client::SERVICE_UNAVAILABLE.to_owned());
        let mut engine = runtime.engine.lock();
        engine.status = CoreStatus::Error;
        engine.last_error = Some(format!(
            "A previous VPN session may still be active, but the service could not be checked: {message}"
        ));
        engine.unresolved_owner = true;
        engine.unresolved_session = Some(clashfork_service_ipc::OwnerSessionProof {
            generation: stored.generation,
            token: stored.token,
        });
    }
    if prime
        && let Some(protocol_info) = probe.protocol_info
        && let Err(error) = runtime.prime_standby(protocol_info).await
    {
        runtime.diagnostics.push(
            "service",
            "warn",
            format!("Warm core priming was skipped: {error}"),
        );
    }
    runtime.emit_snapshot();
    crate::tray::refresh(&runtime).ok();
}

fn recompute_counts(state: &mut AppState) {
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
    for node in &state.nodes {
        let entry = counts.entry(node.service_id.clone()).or_default();
        if node.missing {
            entry.1 += 1;
        } else {
            entry.0 += 1;
        }
    }
    for service in &mut state.services {
        let (available, missing) = counts.get(&service.id).copied().unwrap_or_default();
        service.node_count = available;
        service.missing_count = missing;
    }
}

fn active_ruleset_uses_group(state: &AppState, group_id: &str) -> bool {
    state.selection.mode == Mode::Rule
        && state
            .selection
            .ruleset_id
            .as_ref()
            .and_then(|id| state.rulesets.iter().find(|ruleset| &ruleset.id == id))
            .is_some_and(|ruleset| {
                ruleset
                    .entries
                    .iter()
                    .any(|entry| entry.match_group_id == group_id)
            })
}

fn active_node_references(state: &AppState) -> HashSet<String> {
    let mut ids = HashSet::new();
    if state.selection.mode == Mode::Global {
        if let Some(id) = &state.selection.global_node_id {
            ids.insert(id.clone());
        }
        return ids;
    }
    if let Some(ruleset) = state
        .selection
        .ruleset_id
        .as_ref()
        .and_then(|id| state.rulesets.iter().find(|ruleset| &ruleset.id == id))
    {
        for entry in &ruleset.entries {
            if let crate::models::Destination::Node { node_id } = &entry.destination {
                ids.insert(node_id.clone());
            }
        }
        if let crate::models::Miscellaneous::Node { node_id } = &ruleset.miscellaneous {
            ids.insert(node_id.clone());
        }
    }
    ids
}

fn referenced_node_ids(state: &AppState) -> HashSet<String> {
    let mut ids = active_node_references(state);
    for ruleset in &state.rulesets {
        for entry in &ruleset.entries {
            if let crate::models::Destination::Node { node_id } = &entry.destination {
                ids.insert(node_id.clone());
            }
        }
        if let crate::models::Miscellaneous::Node { node_id } = &ruleset.miscellaneous {
            ids.insert(node_id.clone());
        }
    }
    for preset in &state.presets {
        if let Some(id) = &preset.global_node_id {
            ids.insert(id.clone());
        }
        if let Some(ruleset_id) = &preset.ruleset_id
            && let Some(ruleset) = state.rulesets.iter().find(|item| &item.id == ruleset_id)
        {
            for entry in &ruleset.entries {
                if let crate::models::Destination::Node { node_id } = &entry.destination {
                    ids.insert(node_id.clone());
                }
            }
            if let crate::models::Miscellaneous::Node { node_id } = &ruleset.miscellaneous {
                ids.insert(node_id.clone());
            }
        }
    }
    ids
}

fn omission_is_confirmed(node: &NodeView, pending: &HashSet<String>) -> bool {
    pending.contains(&node.id) || node.missing || node.omitted_once
}

fn reconcile_node_ids(
    service_id: &str,
    old_nodes: &[NodeView],
    incoming: &[crate::models::ParsedNode],
) -> Vec<String> {
    let mut ids: Vec<String> = incoming
        .iter()
        .map(|node| crate::subscription::stable_node_id(service_id, &node.source_id))
        .collect();
    let old_by_id: HashMap<&str, &NodeView> = old_nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let exact_old_ids: HashSet<&str> = ids
        .iter()
        .filter_map(|id| old_by_id.contains_key(id.as_str()).then_some(id.as_str()))
        .collect();

    let key = |name: &str, protocol: &str| {
        (
            name.trim().to_ascii_lowercase(),
            protocol.trim().to_ascii_lowercase(),
        )
    };
    let mut old_by_key: HashMap<(String, String), Vec<&NodeView>> = HashMap::new();
    for old in old_nodes {
        if !exact_old_ids.contains(old.id.as_str()) {
            old_by_key
                .entry(key(&old.display_name, &old.protocol))
                .or_default()
                .push(old);
        }
    }
    let mut incoming_by_key: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (index, node) in incoming.iter().enumerate() {
        if !exact_old_ids.contains(ids[index].as_str()) {
            incoming_by_key
                .entry(key(&node.display_name, &node.protocol))
                .or_default()
                .push(index);
        }
    }
    for (identity, old_matches) in old_by_key {
        let Some(incoming_matches) = incoming_by_key.get(&identity) else {
            continue;
        };
        if old_matches.len() == 1 && incoming_matches.len() == 1 {
            ids[incoming_matches[0]] = old_matches[0].id.clone();
        }
    }
    ids
}

fn aggregate_proxy_rates(deltas: &[(RouteClass, u64, u64)], elapsed: f64) -> (u64, u64) {
    if !elapsed.is_finite() || elapsed <= 0.0 {
        return (0, 0);
    }
    let (download, upload) = deltas
        .iter()
        .fold((0u64, 0u64), |totals, (route, down, up)| {
            if *route == RouteClass::Proxy {
                (totals.0.saturating_add(*down), totals.1.saturating_add(*up))
            } else {
                totals
            }
        });
    (
        (download as f64 / elapsed).round() as u64,
        (upload as f64 / elapsed).round() as u64,
    )
}

fn missing_active_note(state: &AppState, service_id: &str) -> Option<String> {
    let active = active_node_references(state);
    if state
        .nodes
        .iter()
        .any(|node| node.service_id == service_id && node.missing && active.contains(&node.id))
    {
        Some("An active node disappeared from this subscription. Choose a replacement before the next activation.".to_owned())
    } else {
        None
    }
}

const PROVIDER_ACCESS_ERROR: &str = "The provider rejected this subscription. Check whether its access has expired or needs updating.";

fn actionable_refresh_error(error: &anyhow::Error) -> Option<&'static str> {
    match error.to_string().as_str() {
        "The subscription server returned HTTP 401."
        | "The subscription server returned HTTP 403." => Some(PROVIDER_ACCESS_ERROR),
        _ => None,
    }
}

fn normalize_stored_service_errors(state: &mut AppState) {
    let service_ids: Vec<String> = state
        .services
        .iter()
        .map(|service| service.id.clone())
        .collect();
    for service_id in service_ids {
        let missing = missing_active_note(state, &service_id);
        let has_available_nodes = state
            .nodes
            .iter()
            .any(|node| node.service_id == service_id && !node.missing);
        let service = state
            .services
            .iter_mut()
            .find(|service| service.id == service_id)
            .expect("service ID was collected from this state");
        let legacy_access_error = service.last_error.as_deref().is_some_and(|message| {
            matches!(
                message,
                "The subscription server returned HTTP 401."
                    | "The subscription server returned HTTP 403."
            )
        });
        let unavailable = (service.source_type == SourceType::Subscription && !has_available_nodes)
            .then(|| {
                "This service has no available nodes. Restore its subscription or choose another source."
                    .to_owned()
            });
        service.last_error = missing
            .or_else(|| legacy_access_error.then_some(PROVIDER_ACCESS_ERROR.to_owned()))
            .or(unavailable);
    }
}

fn mark_refresh_failed(
    state: &mut AppState,
    service_id: &str,
    error: &anyhow::Error,
) -> Option<String> {
    let has_available_nodes = state
        .nodes
        .iter()
        .any(|node| node.service_id == service_id && !node.missing);
    let message = missing_active_note(state, service_id)
        .or_else(|| actionable_refresh_error(error).map(str::to_owned))
        .or_else(|| {
            (!has_available_nodes).then(|| {
                "This service has no available nodes. Restore its subscription or choose another source."
                    .to_owned()
            })
        });
    if let Some(service) = state
        .services
        .iter_mut()
        .find(|service| service.id == service_id)
    {
        service.stale = true;
        service.last_error = message.clone();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Allowance, AllowanceFull, NodeView, SourceType};

    #[test]
    fn failed_refresh_marks_stale_without_replacing_nodes_or_quota() {
        let mut state = AppState::default();
        state.services.push(ServiceView {
            id: "svc".into(),
            title: "Falemon".into(),
            source_type: SourceType::Subscription,
            source_host: Some("fixture.invalid".into()),
            node_count: 1,
            missing_count: 0,
            allowance: Allowance::Full(AllowanceFull {
                total: 100,
                used: 25,
                remaining: 75,
                expires_at: Some(1_800_000_000),
                source: "header".into(),
            }),
            stale: false,
            omitted_node_ids: Vec::new(),
            last_refresh_at: Some(1_700_000_000),
            last_error: None,
        });
        state.nodes.push(NodeView {
            id: "node-safe".into(),
            service_id: "svc".into(),
            service_title: "Falemon".into(),
            display_name: "Singapore".into(),
            protocol: "Trojan".into(),
            missing: false,
            omitted_once: false,
            core_name: "CF-safe".into(),
            latency_ms: None,
            latency_measured_at_ms: None,
        });
        let original_nodes = serde_json::to_value(&state.nodes).unwrap();
        let original_allowance = serde_json::to_value(&state.services[0].allowance).unwrap();
        let error = anyhow::anyhow!(
            "The subscription request failed for https://user:pass@example.invalid/path-token"
        );

        let message = mark_refresh_failed(&mut state, "svc", &error);

        assert_eq!(serde_json::to_value(&state.nodes).unwrap(), original_nodes);
        assert_eq!(
            serde_json::to_value(&state.services[0].allowance).unwrap(),
            original_allowance
        );
        assert!(state.services[0].stale);
        assert!(message.is_none());
        assert!(state.services[0].last_error.is_none());
    }

    #[test]
    fn failed_refresh_keeps_actionable_access_error_and_missing_node_state() {
        let mut state = AppState::default();
        let mut service = service_view("svc");
        service.last_error = Some("The subscription request timed out.".into());
        state.services.push(service);
        state
            .nodes
            .push(public_node("node-ref", "Singapore", "Trojan"));
        let auth = anyhow::anyhow!("The subscription server returned HTTP 403.");

        let message = mark_refresh_failed(&mut state, "svc", &auth);

        assert_eq!(message.as_deref(), Some(PROVIDER_ACCESS_ERROR));
        assert_eq!(
            state.services[0].last_error.as_deref(),
            Some(PROVIDER_ACCESS_ERROR)
        );
        assert!(state.services[0].stale);
    }

    #[test]
    fn startup_clears_legacy_transient_errors_but_preserves_confirmed_breakage() {
        let mut state = AppState::default();
        let mut service = service_view("svc");
        service.last_error = Some("The subscription response was HTML with sensitive text.".into());
        state.services.push(service);
        let mut node = public_node("node-ref", "Singapore", "Trojan");
        node.missing = true;
        state.nodes.push(node);
        state.selection.mode = Mode::Global;
        state.selection.global_node_id = Some(state.nodes[0].id.clone());

        normalize_stored_service_errors(&mut state);

        assert_eq!(
            state.services[0].last_error.as_deref(),
            Some(
                "An active node disappeared from this subscription. Choose a replacement before the next activation."
            )
        );
    }

    #[test]
    fn startup_clears_old_transient_refresh_error_when_cached_nodes_are_usable() {
        let mut state = AppState::default();
        let mut service = service_view("svc");
        service.last_error = Some("The subscription response was HTML with sensitive text.".into());
        service.stale = true;
        state.services.push(service);
        state
            .nodes
            .push(public_node("node-ref", "Singapore", "Trojan"));

        normalize_stored_service_errors(&mut state);

        assert!(
            state.services[0].stale,
            "the stale fact remains in stored state"
        );
        assert!(state.services[0].last_error.is_none());
    }

    #[test]
    fn failed_refresh_with_no_available_nodes_has_a_safe_actionable_message() {
        let mut state = AppState::default();
        state.services.push(service_view("svc"));
        let mut node = public_node("node-ref", "Singapore", "Trojan");
        node.missing = true;
        state.nodes.push(node);
        let error = anyhow::anyhow!("The subscription request timed out.");

        let message = mark_refresh_failed(&mut state, "svc", &error);

        assert_eq!(
            message.as_deref(),
            Some(
                "This service has no available nodes. Restore its subscription or choose another source."
            )
        );
    }

    fn service_view(id: &str) -> ServiceView {
        ServiceView {
            id: id.into(),
            title: "Fixture".into(),
            source_type: SourceType::Subscription,
            source_host: None,
            node_count: 1,
            missing_count: 0,
            allowance: crate::models::Allowance::Unavailable,
            stale: false,
            omitted_node_ids: Vec::new(),
            last_refresh_at: Some(1_700_000_000),
            last_error: None,
        }
    }

    fn parsed_node(
        source_id: &str,
        name: &str,
        protocol: &str,
        server: &str,
    ) -> crate::models::ParsedNode {
        crate::models::ParsedNode {
            source_id: source_id.to_owned(),
            display_name: name.to_owned(),
            protocol: protocol.to_owned(),
            server: server.to_owned(),
            port: 443,
            secret_material: String::new(),
            proxy: serde_yaml_ng::from_str(&format!(
                "type: {}\nserver: {}\nport: 443\n",
                protocol.to_ascii_lowercase(),
                server
            ))
            .unwrap(),
        }
    }

    fn public_node(source_id: &str, name: &str, protocol: &str) -> NodeView {
        let id = crate::subscription::stable_node_id("svc", source_id);
        NodeView {
            id: id.clone(),
            service_id: "svc".into(),
            service_title: "Fixture".into(),
            display_name: name.into(),
            protocol: protocol.into(),
            missing: false,
            omitted_once: false,
            core_name: crate::subscription::node_core_name(&id),
            latency_ms: None,
            latency_measured_at_ms: None,
        }
    }

    #[test]
    fn node_refresh_reconciliation_prefers_exact_ids_then_unique_rotations() {
        let old = vec![public_node("old-ref", "Singapore", "Trojan")];
        let exact = reconcile_node_ids(
            "svc",
            &old,
            &[parsed_node(
                "old-ref",
                "Singapore renamed",
                "Trojan",
                "edge-a.invalid",
            )],
        );
        assert_eq!(exact[0], old[0].id);

        let rotated = reconcile_node_ids(
            "svc",
            &old,
            &[parsed_node(
                "new-ref",
                "singapore",
                "trojan",
                "edge-b.invalid",
            )],
        );
        assert_eq!(rotated[0], old[0].id);
    }

    #[test]
    fn node_refresh_does_not_guess_ambiguous_or_cross_protocol_matches() {
        let old = vec![
            public_node("old-ref-a", "Singapore", "Trojan"),
            public_node("old-ref-b", "Singapore", "Trojan"),
        ];
        let incoming = [parsed_node(
            "new-ref",
            "Singapore",
            "Trojan",
            "edge.invalid",
        )];
        let reconciled = reconcile_node_ids("svc", &old, &incoming);
        assert_ne!(reconciled[0], old[0].id);
        assert_ne!(reconciled[0], old[1].id);

        let protocol_change = reconcile_node_ids(
            "svc",
            &old[..1],
            &[parsed_node(
                "rotated-ref",
                "Singapore",
                "VLESS",
                "edge.invalid",
            )],
        );
        assert_ne!(protocol_change[0], old[0].id);
    }

    #[test]
    fn first_node_omission_is_pending_and_second_is_confirmed() {
        let node = public_node("source-ref", "Node", "Trojan");
        let pending = HashSet::new();
        assert!(!omission_is_confirmed(&node, &pending));
        let pending = HashSet::from([node.id.clone()]);
        assert!(omission_is_confirmed(&node, &pending));
        let mut legacy_missing = node;
        legacy_missing.missing = true;
        assert!(omission_is_confirmed(&legacy_missing, &HashSet::new()));
    }

    #[test]
    fn saved_inactive_rules_and_presets_protect_referenced_nodes() {
        let mut state = AppState::default();
        state.rulesets.push(crate::models::Ruleset {
            id: "saved-rules".into(),
            name: "Saved".into(),
            entries: vec![crate::models::RulesetEntry {
                id: "saved-entry".into(),
                match_group_id: "group".into(),
                name: String::new(),
                conditions: Vec::new(),
                destination: crate::models::Destination::Node {
                    node_id: "rules-node".into(),
                },
            }],
            miscellaneous: crate::models::Miscellaneous::Node {
                node_id: "fallback-node".into(),
            },
        });
        state.presets.push(crate::models::Preset {
            id: "global-preset".into(),
            name: "Global preset".into(),
            mode: Mode::Global,
            ruleset_id: None,
            global_node_id: Some("preset-node".into()),
        });
        state.presets.push(crate::models::Preset {
            id: "rules-preset".into(),
            name: "Rules preset".into(),
            mode: Mode::Rule,
            ruleset_id: Some("saved-rules".into()),
            global_node_id: None,
        });
        let references = referenced_node_ids(&state);
        assert!(references.contains("rules-node"));
        assert!(references.contains("fallback-node"));
        assert!(references.contains("preset-node"));
    }

    #[test]
    fn dashboard_proxy_rates_exclude_direct_blocked_and_unknown_flows_without_app_metadata() {
        let node_routes =
            BTreeMap::from([("fixture-proxy".to_owned(), "Fixture / Node".to_owned())]);
        let fixtures = [
            (serde_json::json!({"chains": ["fixture-proxy"]}), 1_200, 400),
            (serde_json::json!({"chains": ["DIRECT"]}), 9_000, 300),
            (serde_json::json!({"chains": ["REJECT"]}), 800, 90),
            (serde_json::json!({"chains": []}), 2_000, 120),
        ];
        let deltas = fixtures
            .iter()
            .map(|(connection, download, upload)| {
                (
                    route_from_chains(&node_routes, connection).class,
                    *download,
                    *upload,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(aggregate_proxy_rates(&deltas, 2.0), (600, 200));
    }
}

fn match_groups_by_core(state: &AppState) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if state.selection.mode != Mode::Rule {
        return map;
    }
    let Some(ruleset) = state
        .selection
        .ruleset_id
        .as_ref()
        .and_then(|id| state.rulesets.iter().find(|ruleset| &ruleset.id == id))
    else {
        return map;
    };
    for entry in &ruleset.entries {
        if let crate::models::Destination::Node { node_id } = &entry.destination
            && let Some(node) = state.nodes.iter().find(|node| &node.id == node_id)
        {
            let label = if entry.name.is_empty() {
                state
                    .match_groups
                    .iter()
                    .find(|group| group.id == entry.match_group_id)
                    .map(|group| group.name.as_str())
                    .unwrap_or("Rule")
            } else {
                entry.name.as_str()
            };
            map.entry(node.core_name.clone())
                .or_insert(label.to_owned());
        }
    }
    if let crate::models::Miscellaneous::Node { node_id } = &ruleset.miscellaneous
        && let Some(node) = state.nodes.iter().find(|node| &node.id == node_id)
    {
        map.entry(node.core_name.clone())
            .or_insert_with(|| "Everything else".to_owned());
    }
    map
}

fn sanitize_service_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        bail!("The service name cannot be empty.");
    }
    if title.chars().count() > 80 {
        bail!("The service name must be 80 characters or fewer.");
    }
    if title.chars().any(|c| c.is_control()) {
        bail!("The service name cannot contain control characters.");
    }
    Ok(title.to_owned())
}

fn sanitize_group_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("The group name cannot be empty.");
    }
    if name.chars().count() > 80 {
        bail!("The group name must be 80 characters or fewer.");
    }
    if name.chars().any(|c| c.is_control()) {
        bail!("The group name cannot contain control characters.");
    }
    Ok(name.to_owned())
}

fn pick_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn random_hex(bytes: usize) -> String {
    let mut output = String::with_capacity(bytes * 2);
    while output.len() < bytes * 2 {
        output.push_str(&Uuid::new_v4().simple().to_string());
    }
    output.truncate(bytes * 2);
    output
}

const TUN_ROUTE_READY_TIMEOUT: Duration = Duration::from_secs(6);
const TUN_ROUTE_RELEASE_TIMEOUT: Duration = Duration::from_secs(2);
const TUN_ROUTE_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);

fn can_retain_warm_core(controller_confirms_tun_off: bool, routes_absent: Option<bool>) -> bool {
    controller_confirms_tun_off && routes_absent == Some(true)
}

async fn wait_for_tun_routes_ready(
    controller: &CoreController,
    expected_mode: &str,
) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + TUN_ROUTE_READY_TIMEOUT;
    let mut stable_samples = 0;
    let mut competing_since = None;
    let mut indeterminate_samples = 0;
    let mut controller_failures = 0;
    let mut last = None;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let config_status = tokio::time::timeout(
            remaining.min(Duration::from_millis(500)),
            controller.config_status(),
        )
        .await;
        let config_ready = match config_status {
            Ok(Ok(status)) => {
                controller_failures = 0;
                status.mode.eq_ignore_ascii_case(expected_mode) && status.tun_enabled == Some(true)
            }
            Ok(Err(_)) | Err(_) => {
                controller_failures += 1;
                if controller_failures >= 3 {
                    bail!(
                        "Mihomo stopped confirming the active mode and TUN state during Windows route handoff."
                    );
                }
                false
            }
        };
        if !config_ready {
            stable_samples = 0;
            indeterminate_samples = 0;
            competing_since = None;
            let now = tokio::time::Instant::now();
            if now < deadline {
                tokio::time::sleep((deadline - now).min(TUN_ROUTE_SAMPLE_INTERVAL)).await;
            }
            continue;
        }

        let probe = crate::network::probe_tun_routes();
        last = Some(probe.readiness);
        match probe.readiness {
            crate::network::TunRouteReadiness::Ready => {
                stable_samples += 1;
                competing_since = None;
                if stable_samples >= 2 {
                    return Ok(true);
                }
            }
            crate::network::TunRouteReadiness::CompetingTunnel => {
                stable_samples = 0;
                indeterminate_samples = 0;
                let since = competing_since.get_or_insert_with(tokio::time::Instant::now);
                if since.elapsed() >= Duration::from_secs(2) {
                    bail!(
                        "Another active tunnel still owns a public route. Clashfork did not report the VPN as connected."
                    );
                }
            }
            crate::network::TunRouteReadiness::Pending => {
                stable_samples = 0;
                indeterminate_samples = 0;
                competing_since = None;
            }
            crate::network::TunRouteReadiness::Indeterminate
            | crate::network::TunRouteReadiness::Unsupported => {
                stable_samples = 0;
                indeterminate_samples += 1;
                competing_since = None;
                if indeterminate_samples >= 3 {
                    return Ok(false);
                }
            }
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        tokio::time::sleep((deadline - now).min(TUN_ROUTE_SAMPLE_INTERVAL)).await;
    }

    match last.unwrap_or(crate::network::TunRouteReadiness::Pending) {
        crate::network::TunRouteReadiness::CompetingTunnel => bail!(
            "Another active tunnel still owns a public route. Clashfork did not report the VPN as connected."
        ),
        crate::network::TunRouteReadiness::Indeterminate
        | crate::network::TunRouteReadiness::Unsupported => Ok(false),
        crate::network::TunRouteReadiness::Pending | crate::network::TunRouteReadiness::Ready => {
            bail!(
                "Windows did not confirm that public routes had settled on ClashforkTUN. The VPN session was stopped."
            )
        }
    }
}

async fn wait_for_tun_routes_disabled() -> Option<bool> {
    let deadline = tokio::time::Instant::now() + TUN_ROUTE_RELEASE_TIMEOUT;
    let mut last_known;
    loop {
        let probe = crate::network::probe_tun_routes();
        match probe.has_owned_route {
            Some(false) => return Some(true),
            Some(true) => last_known = Some(false),
            None => last_known = None,
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return last_known;
        }
        tokio::time::sleep((deadline - now).min(TUN_ROUTE_SAMPLE_INTERVAL)).await;
    }
}

async fn sampler_loop(runtime: Arc<AppRuntime>) {
    let mut prune_at = 0i64;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let now = Utc::now().timestamp();
        if prune_at == 0 || now >= prune_at {
            prune_at = now + 3600;
            if let Err(error) = runtime.history.lock().prune(now) {
                runtime.diagnostics.push(
                    "app",
                    "warn",
                    format!("History retention failed: {error}"),
                );
            }
        }
        if runtime.current_active().is_none() {
            let mut live = runtime.live.lock();
            live.active = false;
            live.available = false;
            live.proxy_download_rate = 0;
            live.proxy_upload_rate = 0;
            live.connections.clear();
            live.flow_counters.clear();
            continue;
        }
        if prune_at == 0 || now % 5 == 0 {
            if let Ok(credentials) = security::current_owner_credentials(&runtime.paths.data_root)
                && let Ok(response) = clashfork_service_ipc::get_clash_logs(&credentials).await
                && response.code == 0
                && let Some(lines) = response.data
            {
                let mut seen = runtime.core_logs.lock();
                for line in lines.into_iter().rev().take(12).rev() {
                    if seen.contains(&line) {
                        continue;
                    }
                    runtime.diagnostics.push(
                        "core",
                        if line.to_ascii_lowercase().contains("error") {
                            "error"
                        } else {
                            "info"
                        },
                        &line,
                    );
                    seen.push_back(line);
                }
                while seen.len() > 160 {
                    seen.pop_front();
                }
            }
        }
        let _sample_guard = runtime.sample_gate.read().await;
        let Some(active) = runtime.current_active() else {
            let mut live = runtime.live.lock();
            live.active = false;
            live.available = false;
            live.proxy_download_rate = 0;
            live.proxy_upload_rate = 0;
            live.connections.clear();
            live.flow_counters.clear();
            continue;
        };
        let sample_generation = runtime.engine.lock().generation;
        let paused = runtime.state.lock().history_paused;
        let response = active.core.controller.connections().await;
        match response {
            Ok(value) => {
                let elapsed = {
                    let mut live = runtime.live.lock();
                    let elapsed = live
                        .last_sample_at
                        .map(|instant| instant.elapsed().as_secs_f64().max(0.25))
                        .unwrap_or(1.0);
                    live.last_sample_at = Some(Instant::now());
                    elapsed
                };
                let totals = value
                    .get("downloadTotal")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let upload_totals = value
                    .get("uploadTotal")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let mut reset = false;
                let (down_delta, up_delta) = {
                    let mut live = runtime.live.lock();
                    let delta = match live.last_totals {
                        Some((previous_down, previous_up))
                            if totals >= previous_down && upload_totals >= previous_up =>
                        {
                            (totals - previous_down, upload_totals - previous_up)
                        }
                        Some(_) => {
                            reset = true;
                            (0, 0)
                        }
                        None => (0, 0),
                    };
                    live.last_totals = Some((totals, upload_totals));
                    live.download_total = totals;
                    live.upload_total = upload_totals;
                    live.download_rate = if reset {
                        0
                    } else {
                        (delta.0 as f64 / elapsed).round() as u64
                    };
                    live.upload_rate = if reset {
                        0
                    } else {
                        (delta.1 as f64 / elapsed).round() as u64
                    };
                    delta
                };
                if reset {
                    let mut history = runtime.history.lock();
                    if !paused {
                        let _ = history.mark_gap(now);
                    }
                    runtime.diagnostics.push(
                        "app",
                        "warn",
                        "Mihomo counters restarted; a traffic history gap was recorded.",
                    );
                }
                let connections = value
                    .get("connections")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut flow_deltas: Vec<(String, String, RouteClass, u64, u64)> = Vec::new();
                let mut proxy_deltas: Vec<(RouteClass, u64, u64)> = Vec::new();
                let mut live_routes = Vec::new();
                let mut previous_flows: HashMap<String, (u64, u64)> = HashMap::new();
                {
                    let mut live = runtime.live.lock();
                    for connection in &connections {
                        let Some(id) = connection.get("id").and_then(serde_json::Value::as_str)
                        else {
                            continue;
                        };
                        let download = connection
                            .get("download")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                        let upload = connection
                            .get("upload")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                        let (previous_down, previous_up) = live
                            .flow_counters
                            .get(id)
                            .copied()
                            .unwrap_or((download, upload));
                        let flow_down = download.saturating_sub(previous_down);
                        let flow_up = upload.saturating_sub(previous_up);
                        let route = route_for(&active, connection);
                        if route.class == RouteClass::Proxy {
                            proxy_deltas.push((route.class, flow_down, flow_up));
                        }
                        if flow_down > 0 || flow_up > 0 {
                            let app = app_for(connection);
                            if let Some(app) = &app {
                                flow_deltas.push((
                                    app.clone(),
                                    route.label.clone(),
                                    route.class,
                                    flow_down,
                                    flow_up,
                                ));
                            }
                            live_routes.push(LiveRoute {
                                app: app.unwrap_or_else(|| "Unattributed".to_owned()),
                                app_key: id.to_owned(),
                                destination: destination_for(connection),
                                network: connection
                                    .pointer("/metadata/network")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or("tcp")
                                    .to_owned(),
                                route: route.label,
                                matched_group: matched_group_for(&active, connection),
                                download_rate: (flow_down as f64 / elapsed).round() as u64,
                                upload_rate: (flow_up as f64 / elapsed).round() as u64,
                                download_total: download,
                                upload_total: upload,
                                started_at: started_at(connection),
                            });
                        } else {
                            let app =
                                app_for(connection).unwrap_or_else(|| "Unattributed".to_owned());
                            live_routes.push(LiveRoute {
                                app,
                                app_key: id.to_owned(),
                                destination: destination_for(connection),
                                network: connection
                                    .pointer("/metadata/network")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or("tcp")
                                    .to_owned(),
                                route: route.label,
                                matched_group: matched_group_for(&active, connection),
                                download_rate: 0,
                                upload_rate: 0,
                                download_total: download,
                                upload_total: upload,
                                started_at: started_at(connection),
                            });
                        }
                        previous_flows.insert(id.to_owned(), (download, upload));
                    }
                    live.connections = live_routes;
                    live.flow_counters = previous_flows;
                    live.sampled_at = now;
                    live.generation = sample_generation;
                    let (download, upload) = aggregate_proxy_rates(&proxy_deltas, elapsed);
                    live.proxy_download_rate = if reset { 0 } else { download };
                    live.proxy_upload_rate = if reset { 0 } else { upload };
                    live.available = true;
                }
                if !paused {
                    let mut history = runtime.history.lock();
                    if let Err(error) = history.record_totals(now, down_delta, up_delta) {
                        runtime.diagnostics.push(
                            "app",
                            "warn",
                            format!("Traffic total history write failed: {error}"),
                        );
                    }
                    if let Err(error) = history.record_apps(now, &flow_deltas) {
                        runtime.diagnostics.push(
                            "app",
                            "warn",
                            format!("Traffic app history write failed: {error}"),
                        );
                    }
                }
                {
                    let mut live = runtime.live.lock();
                    live.active = true;
                    live.available = true;
                    live.gap_marked = false;
                }
            }
            Err(error) => {
                let should_mark = {
                    let mut live = runtime.live.lock();
                    live.active = true;
                    live.available = false;
                    live.proxy_download_rate = 0;
                    live.proxy_upload_rate = 0;
                    live.download_rate = 0;
                    live.upload_rate = 0;
                    live.connections.clear();
                    live.flow_counters.clear();
                    if live.gap_marked {
                        false
                    } else {
                        live.gap_marked = true;
                        true
                    }
                };
                if should_mark {
                    if !paused {
                        let _ = runtime.history.lock().mark_gap(now);
                    }
                    runtime.diagnostics.push(
                        "core",
                        "warn",
                        format!("Mihomo flow snapshot unavailable; history gap recorded: {error}"),
                    );
                }
            }
        }
    }
}

fn app_for(connection: &serde_json::Value) -> Option<String> {
    let metadata = connection.get("metadata")?;
    for key in ["processPath", "process"] {
        if let Some(value) = metadata.get(key).and_then(serde_json::Value::as_str) {
            let name = std::path::Path::new(value)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| value.to_owned());
            if !name.is_empty() {
                return Some(truncate_ui(&name, 90));
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteInfo {
    label: String,
    class: RouteClass,
}

fn route_for(active: &ActiveCore, connection: &serde_json::Value) -> RouteInfo {
    route_from_chains(&active.node_routes, connection)
}

fn route_from_chains(
    node_routes: &BTreeMap<String, String>,
    connection: &serde_json::Value,
) -> RouteInfo {
    let chains: Vec<String> = connection
        .get("chains")
        .and_then(serde_json::Value::as_array)
        .map(|chains| {
            chains
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if chains
        .iter()
        .any(|chain| chain.eq_ignore_ascii_case("REJECT"))
    {
        return RouteInfo {
            label: "Blocked".to_owned(),
            class: RouteClass::Blocked,
        };
    }
    for chain in &chains {
        if let Some(route) = node_routes.get(chain) {
            return RouteInfo {
                label: route.clone(),
                class: RouteClass::Proxy,
            };
        }
    }
    if chains
        .iter()
        .any(|chain| chain.eq_ignore_ascii_case("DIRECT"))
    {
        return RouteInfo {
            label: "Direct".to_owned(),
            class: RouteClass::Direct,
        };
    }
    RouteInfo {
        label: if chains.is_empty() {
            "Unattributed".to_owned()
        } else {
            truncate_ui(&chains.join(" → "), 90)
        },
        class: RouteClass::Unknown,
    }
}

fn matched_group_for(active: &ActiveCore, connection: &serde_json::Value) -> Option<String> {
    for chain in connection
        .get("chains")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        if let Some(group) = active.match_groups.get(chain) {
            return Some(group.clone());
        }
    }
    None
}

fn destination_for(connection: &serde_json::Value) -> String {
    let metadata = connection.get("metadata");
    if let Some(host) = metadata
        .and_then(|metadata| metadata.get("host"))
        .and_then(serde_json::Value::as_str)
        .filter(|host| !host.is_empty())
    {
        return truncate_ui(host, 120);
    }
    if let Some(remote) = metadata
        .and_then(|metadata| metadata.get("remoteDestination"))
        .and_then(serde_json::Value::as_str)
        .filter(|remote| !remote.is_empty())
    {
        return truncate_ui(remote, 120);
    }
    let ip = metadata
        .and_then(|metadata| metadata.get("destinationIP"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let port = metadata
        .and_then(|metadata| metadata.get("destinationPort"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    truncate_ui(
        &format!(
            "{ip}{}",
            if port.is_empty() {
                String::new()
            } else {
                format!(":{port}")
            }
        ),
        120,
    )
}

fn started_at(connection: &serde_json::Value) -> i64 {
    connection
        .get("start")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp())
        .unwrap_or_else(|| Utc::now().timestamp())
}

fn truncate_ui(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    value.chars().take(max).collect()
}

pub fn setup_window_events(app: &tauri::AppHandle, runtime: Arc<AppRuntime>) {
    if let Some(window) = app.get_webview_window("main") {
        let runtime_for_event = runtime.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if runtime_for_event.quit_requested.load(Ordering::SeqCst) {
                    return;
                }
                api.prevent_close();
                if let Some(window) = runtime_for_event.app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
        });
    }
}

#[cfg(test)]
mod route_class_tests {
    use super::*;

    #[test]
    fn connection_routes_have_explicit_classes_and_missing_chains_are_unknown() {
        let routes = BTreeMap::from([("CF-hk".to_owned(), "Falemon / Hong Kong".to_owned())]);

        let proxy = route_from_chains(&routes, &serde_json::json!({"chains": ["Global", "CF-hk"]}));
        assert_eq!(proxy.label, "Falemon / Hong Kong");
        assert_eq!(proxy.class, RouteClass::Proxy);

        let direct = route_from_chains(&routes, &serde_json::json!({"chains": ["DIRECT"]}));
        assert_eq!(direct.label, "Direct");
        assert_eq!(direct.class, RouteClass::Direct);

        let blocked = route_from_chains(&routes, &serde_json::json!({"chains": ["REJECT"]}));
        assert_eq!(blocked.label, "Blocked");
        assert_eq!(blocked.class, RouteClass::Blocked);

        let unknown = route_from_chains(&routes, &serde_json::json!({}));
        assert_eq!(unknown.label, "Unattributed");
        assert_eq!(unknown.class, RouteClass::Unknown);
    }

    #[test]
    fn warm_core_is_retained_only_after_controller_and_windows_confirm_tun_off() {
        assert!(can_retain_warm_core(true, Some(true)));
        assert!(!can_retain_warm_core(false, Some(true)));
        assert!(!can_retain_warm_core(true, Some(false)));
        assert!(!can_retain_warm_core(true, None));
    }
}

// The batching trait above keeps the call site readable.
