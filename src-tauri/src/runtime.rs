use crate::{
    controller::CoreController,
    diagnostics::Diagnostics,
    history::HistoryStore,
    models::{
        AppState, CoreStatus, CustomAppGroup, HistoryInput, HistoryReport, ImportPreview,
        LiveRoute, LiveSnapshot, Mode, NodeView, PendingImport, RouteClass, RuntimeView,
        SecretStore, ServiceView, Snapshot, SourceType,
    },
    paths::AppPaths,
    rules::{self, ControllerConfig},
    security, service_client, store,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use parking_lot::Mutex;
use reqwest::Client;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

#[derive(Clone)]
pub struct ActiveCore {
    session: clashfork_service_ipc::OwnerSessionProof,
    controller: CoreController,
    controller_config: ControllerConfig,
    yaml: String,
    summary: String,
    mode: Mode,
    node_routes: BTreeMap<String, String>,
    match_groups: BTreeMap<String, String>,
}

pub struct EngineState {
    status: CoreStatus,
    service_available: bool,
    service_message: Option<String>,
    last_error: Option<String>,
    active: Option<ActiveCore>,
    generation: u64,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            status: CoreStatus::Off,
            service_available: false,
            service_message: None,
            last_error: None,
            active: None,
            generation: 0,
        }
    }
}

#[derive(Default)]
pub struct LiveState {
    pub active: bool,
    pub download_rate: u64,
    pub upload_rate: u64,
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
    pub app: AppHandle,
    pub quit_requested: std::sync::atomic::AtomicBool,
    pub generation: AtomicU64,
}

impl AppRuntime {
    pub fn new(app: AppHandle) -> Result<Arc<Self>> {
        let paths = AppPaths::discover()?;
        paths.cleanup_runtime();
        let state = store::load_state(&paths)?;
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
            app,
            quit_requested: std::sync::atomic::AtomicBool::new(false),
            generation: AtomicU64::new(1),
        }))
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.state.lock().clone();
        recompute_counts(&mut state);
        let engine = self.engine.lock();
        let active = engine.active.as_ref();
        Snapshot {
            state,
            runtime: RuntimeView {
                vpn_active: active.is_some() && engine.status == CoreStatus::On,
                core_status: engine.status,
                service_available: engine.service_available,
                service_message: engine.service_message.clone(),
                last_error: engine.last_error.clone(),
                active_summary: active.map(|active| active.summary.clone()),
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

    async fn enable_vpn(self: &Arc<Self>) -> Result<()> {
        {
            let engine = self.engine.lock();
            if engine.active.is_some() {
                return Ok(());
            }
        }
        let probe = service_client::probe().await;
        {
            let mut engine = self.engine.lock();
            engine.service_available = probe.available;
            engine.service_message.clone_from(&probe.message);
            if !probe.available {
                engine.last_error = probe.message.clone();
                engine.status = CoreStatus::Error;
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
        let (state, secrets) = (self.state.lock().clone(), self.secrets.lock().clone());
        if let Err(error) = rules::selection_is_valid(&state, &state.selection) {
            self.fail_start(&error);
            return Err(error);
        }
        let controller_config = ControllerConfig {
            port: pick_port()?,
            secret: random_hex(32),
            dns_port: pick_port()?,
        };
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
                return Err(error);
            }
        };
        if let Err(error) = self.validate_config(&compiled.yaml).await {
            self.fail_start(&error);
            return Err(error);
        }
        let session_token = service_client::new_session_token();
        let started = match service_client::start(
            &self.paths,
            compiled.yaml.clone(),
            &controller_config,
            session_token.clone(),
        )
        .await
        {
            Ok(started) => started,
            Err(error) => {
                self.fail_start(&error);
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
            self.fail_start(&error);
            let _ = service_client::stop(&self.paths, &proof).await;
            return Err(error.context("the VPN session could not be recorded safely"));
        }
        let controller = CoreController::new(
            controller_config.port,
            controller_config.secret.clone(),
            self.http.clone(),
        );
        if let Err(error) = service_client::wait_ready(&controller).await {
            let _ = service_client::stop(&self.paths, &proof).await;
            let mut secrets = self.secrets.lock();
            secrets.last_session = None;
            let _ = store::save_secrets(&self.paths, &secrets);
            self.fail_start(&error);
            return Err(error);
        }
        if let Err(error) = self.verify_mode(&controller, state.selection.mode).await {
            let _ = service_client::stop(&self.paths, &proof).await;
            let mut secrets = self.secrets.lock();
            secrets.last_session = None;
            let _ = store::save_secrets(&self.paths, &secrets);
            self.fail_start(&error);
            return Err(error);
        }
        let summary =
            rules::active_summary(&state, &state.selection).unwrap_or_else(|_| "Connected".into());
        let match_groups = match_groups_by_core(&state);
        let active = ActiveCore {
            session: proof,
            controller,
            controller_config,
            yaml: compiled.yaml,
            summary: summary.clone(),
            mode: state.selection.mode,
            node_routes: compiled.node_routes,
            match_groups,
        };
        let mut engine = self.engine.lock();
        engine.active = Some(active);
        engine.status = CoreStatus::On;
        engine.last_error = None;
        engine.generation = self.generation.fetch_add(1, Ordering::SeqCst);
        let generation = engine.generation;
        drop(engine);
        {
            let mut live = self.live.lock();
            *live = LiveState {
                active: true,
                generation,
                ..LiveState::default()
            };
        }
        self.diagnostics.push(
            "app",
            "info",
            format!("VPN enabled with TUN adapter {}.", rules::TUN_DEVICE),
        );
        let _ = summary;
        Ok(())
    }

    fn fail_start(&self, error: &anyhow::Error) {
        let mut engine = self.engine.lock();
        engine.status = CoreStatus::Error;
        engine.last_error = Some(error.to_string());
        self.diagnostics
            .push("app", "error", format!("VPN start failed: {error}"));
    }

    async fn disable_vpn(self: &Arc<Self>) -> Result<()> {
        let active = { self.engine.lock().active.clone() };
        let Some(active) = active else {
            let mut engine = self.engine.lock();
            engine.status = CoreStatus::Off;
            engine.last_error = None;
            return Ok(());
        };
        match service_client::stop(&self.paths, &active.session).await {
            Ok(()) => {}
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
        {
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
        {
            let mut engine = self.engine.lock();
            engine.active = None;
            engine.status = CoreStatus::Off;
            engine.last_error = None;
            engine.generation = self.generation.fetch_add(1, Ordering::SeqCst);
        }
        {
            let mut live = self.live.lock();
            *live = LiveState::default();
        }
        self.paths.cleanup_runtime();
        self.diagnostics
            .push("app", "info", "VPN disabled and owned TUN stopped.");
        Ok(())
    }

    pub async fn shutdown(self: &Arc<Self>) {
        self.quit_requested.store(true, Ordering::SeqCst);
        let _guard = self.apply_gate.lock().await;
        let active = { self.engine.lock().active.clone() };
        if let Some(active) = active
            && let Err(error) = service_client::stop(&self.paths, &active.session).await
        {
            self.diagnostics.push(
                "app",
                "warn",
                format!("VPN cleanup at exit failed: {error}"),
            );
        }
        let mut secrets = self.secrets.lock();
        secrets.last_session = None;
        let _ = store::save_secrets(&self.paths, &secrets);
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
            &active.controller_config,
            true,
        )?;
        self.validate_config(&compiled.yaml).await?;
        let staged =
            service_client::stage(&self.paths, compiled.yaml.clone(), &active.session).await?;
        let config_path = match staged {
            service_client::Staged::Ready(path) => path,
            service_client::Staged::Restart => {
                bail!(
                    "The Clashfork service requires a full restart to apply this routing change. The current routing was kept."
                );
            }
        };
        if let Err(error) = active.controller.reload(&config_path).await {
            return Err(error.context(self.rollback_runtime(&active).await));
        }
        if let Err(error) = self
            .verify_mode(&active.controller, candidate.selection.mode)
            .await
        {
            return Err(error.context(self.rollback_runtime(&active).await));
        }
        if let Err(error) = active.controller.close_connections().await {
            self.diagnostics.push(
                "app",
                "warn",
                format!("Could not close all existing flows after routing switch: {error}"),
            );
        }
        let summary = rules::active_summary(candidate, &candidate.selection)?;
        let match_groups = match_groups_by_core(candidate);
        {
            let mut engine = self.engine.lock();
            if let Some(active_state) = engine.active.as_mut() {
                active_state.yaml = compiled.yaml;
                active_state.summary = summary;
                active_state.mode = candidate.selection.mode;
                active_state.node_routes = compiled.node_routes;
                active_state.match_groups = match_groups;
            }
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
            service_client::stage(&self.paths, active.yaml.clone(), &active.session).await
            && active.controller.reload(&path).await.is_ok()
            && self
                .verify_mode(&active.controller, active.mode)
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
        if let Err(error) = service_client::stop(&self.paths, &active.session).await {
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
        let result = tokio::time::timeout(
            Duration::from_secs(25),
            tokio::process::Command::new(&core)
                .arg("-t")
                .arg("-d")
                .arg(&check_dir)
                .arg("-f")
                .arg(&config_path)
                .output(),
        )
        .await;
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
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for mut entry in input.entries {
            if !seen.insert(entry.match_group_id.clone()) {
                bail!("A match group can appear only once in a ruleset.");
            }
            if !candidate
                .match_groups
                .iter()
                .any(|group| group.id == entry.match_group_id)
            {
                bail!("A ruleset references a missing match group.");
            }
            if entry.id.trim().is_empty() {
                entry.id = rules::generate_id("entry");
            }
            if let crate::models::Destination::Node { node_id } = &entry.destination {
                rules::validate_node_reference(&candidate, node_id)?;
            }
            entries.push(entry);
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
        let _gate = self.apply_gate.lock().await;
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
        let mut candidate = self.state.lock().clone();
        let mut secrets = self.secrets.lock().clone();
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
                let active_refs = active_node_references(&candidate);
                let mut new_ids = HashSet::new();
                let mut new_public = Vec::new();
                let mut new_secrets = Vec::new();
                for node in source.nodes {
                    let id = crate::subscription::stable_node_id(service_id, &node.source_id);
                    new_ids.insert(id.clone());
                    new_public.push(NodeView {
                        id: id.clone(),
                        service_id: service_id.to_owned(),
                        service_title: service.title.clone(),
                        display_name: node.display_name,
                        protocol: node.protocol,
                        missing: false,
                        core_name: crate::subscription::node_core_name(&id),
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
                for old in old_nodes {
                    if active_refs.contains(&old.id) && !new_ids.contains(&old.id) {
                        let mut missing = old;
                        missing.missing = true;
                        if let Some(proxy) = old_defs.get(&missing.id) {
                            secrets.nodes.insert(missing.id.clone(), proxy.clone());
                        }
                        candidate.nodes.push(missing);
                    }
                }
                let active_note = missing_active_note(&candidate, &service_id);
                if let Some(service) = candidate
                    .services
                    .iter_mut()
                    .find(|service| service.id == service_id)
                {
                    service.allowance = source.allowance;
                    service.stale = false;
                    service.last_refresh_at = Some(Utc::now().timestamp());
                    service.last_error = active_note;
                    service.source_host = source.source_host;
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
                mark_refresh_failed(&mut candidate, service_id, &error);
                store::save_state(&self.paths, &candidate)?;
                *self.state.lock() = candidate;
                self.diagnostics.push(
                    "app",
                    "warn",
                    format!("Subscription refresh failed; last good nodes and allowance were retained: {error}"),
                );
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
        history.report(
            &input.range,
            &input.breakdown,
            &groups,
            Utc::now().timestamp(),
        )
    }

    pub fn live_snapshot(&self) -> LiveSnapshot {
        let live = self.live.lock();
        LiveSnapshot {
            active: live.active,
            download_rate: live.download_rate,
            upload_rate: live.upload_rate,
            download_total: live.download_total,
            upload_total: live.upload_total,
            connections: live.connections.clone(),
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
    let probe = service_client::probe().await;
    {
        let mut engine = runtime.engine.lock();
        engine.service_available = probe.available;
        engine.service_message = probe.message;
    }
    if probe.available {
        let mut secrets = runtime.secrets.lock().clone();
        match service_client::already_owned_session(&runtime.paths, &mut secrets).await {
            Ok(Some(message)) => runtime.diagnostics.push("service", "warn", message),
            Ok(None) => {
                if let Err(error) = store::save_secrets(&runtime.paths, &secrets) {
                    runtime.diagnostics.push(
                        "app",
                        "warn",
                        format!("Could not clear the stale VPN session: {error}"),
                    );
                } else {
                    *runtime.secrets.lock() = secrets;
                }
            }
            Err(error) => runtime.diagnostics.push(
                "service",
                "warn",
                format!("Previous session cleanup could not run: {error}"),
            ),
        }
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
    }
    ids
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

fn public_refresh_error(error: &anyhow::Error) -> String {
    let message = error.to_string();
    let message = crate::diagnostics::redact(&message);
    if message.chars().count() > 200 {
        format!("{}…", message.chars().take(200).collect::<String>())
    } else {
        message
    }
}

fn mark_refresh_failed(state: &mut AppState, service_id: &str, error: &anyhow::Error) {
    if let Some(service) = state
        .services
        .iter_mut()
        .find(|service| service.id == service_id)
    {
        service.stale = true;
        service.last_error = Some(public_refresh_error(error));
    }
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
            core_name: "CF-safe".into(),
        });
        let original_nodes = serde_json::to_value(&state.nodes).unwrap();
        let original_allowance = serde_json::to_value(&state.services[0].allowance).unwrap();
        let error = anyhow::anyhow!(
            "The subscription request failed for https://user:pass@example.invalid/path-token"
        );

        mark_refresh_failed(&mut state, "svc", &error);

        assert_eq!(serde_json::to_value(&state.nodes).unwrap(), original_nodes);
        assert_eq!(
            serde_json::to_value(&state.services[0].allowance).unwrap(),
            original_allowance
        );
        assert!(state.services[0].stale);
        let last_error = state.services[0].last_error.as_deref().unwrap();
        assert!(last_error.contains("[redacted-url]"));
        assert!(!last_error.contains("path-token"));
        assert!(!last_error.contains("user:pass"));
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
            && let Some(group) = state
                .match_groups
                .iter()
                .find(|group| group.id == entry.match_group_id)
        {
            map.entry(node.core_name.clone())
                .or_insert(group.name.clone());
        }
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
        let active = runtime.current_active();
        let Some(active) = active else {
            let mut live = runtime.live.lock();
            live.active = false;
            continue;
        };
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
        let paused = runtime.state.lock().history_paused;
        let response = active.controller.connections().await;
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
                        if flow_down > 0 || flow_up > 0 {
                            let route = route_for(&active, connection);
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
                            let route = route_for(&active, connection);
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
                    live.generation = active.session.generation;
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
                    live.gap_marked = false;
                }
            }
            Err(error) => {
                let should_mark = {
                    let mut live = runtime.live.lock();
                    live.active = true;
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
}

// The batching trait above keeps the call site readable.
