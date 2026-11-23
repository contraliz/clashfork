use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Rule,
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceFull {
    pub total: u64,
    pub used: u64,
    pub remaining: u64,
    pub expires_at: Option<i64>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceRemaining {
    pub remaining: u64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Allowance {
    Full(AllowanceFull),
    Remaining(AllowanceRemaining),
    Unavailable,
}

impl Default for Allowance {
    fn default() -> Self {
        Self::Unavailable
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    pub id: String,
    pub title: String,
    pub source_type: SourceType,
    pub source_host: Option<String>,
    pub node_count: usize,
    pub missing_count: usize,
    #[serde(default)]
    pub allowance: Allowance,
    #[serde(default)]
    pub stale: bool,
    /// Referenced nodes omitted from the most recent successful source fetch.
    #[serde(default)]
    pub omitted_node_ids: Vec<String>,
    pub last_refresh_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceType {
    Subscription,
    Link,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeView {
    pub id: String,
    pub service_id: String,
    pub service_title: String,
    pub display_name: String,
    pub protocol: String,
    pub missing: bool,
    #[serde(default)]
    pub omitted_once: bool,
    pub core_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_measured_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionKind {
    ProcessName,
    ProcessPath,
    Domain,
    DomainSuffix,
    Ipv4Cidr,
    Ipv6Cidr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchCondition {
    pub id: String,
    pub kind: ConditionKind,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchGroup {
    pub id: String,
    pub name: String,
    pub conditions: Vec<MatchCondition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Destination {
    Direct,
    Block,
    Node {
        #[serde(alias = "node_id")]
        node_id: String,
    },
}

impl Default for Destination {
    fn default() -> Self {
        Self::Direct
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesetEntry {
    pub id: String,
    #[serde(default)]
    pub match_group_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub conditions: Vec<MatchCondition>,
    pub destination: Destination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Miscellaneous {
    Direct,
    Block,
    Node { node_id: String },
}

impl Default for Miscellaneous {
    fn default() -> Self {
        Self::Direct
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum LegacyMiscellaneous {
    Direct,
    Block,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MiscellaneousInput {
    Legacy(LegacyMiscellaneous),
    Destination(Destination),
}

impl Serialize for Miscellaneous {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Direct => Destination::Direct,
            Self::Block => Destination::Block,
            Self::Node { node_id } => Destination::Node {
                node_id: node_id.clone(),
            },
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Miscellaneous {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match MiscellaneousInput::deserialize(deserializer)? {
            MiscellaneousInput::Legacy(LegacyMiscellaneous::Direct) => Ok(Self::Direct),
            MiscellaneousInput::Legacy(LegacyMiscellaneous::Block) => Ok(Self::Block),
            MiscellaneousInput::Destination(Destination::Direct) => Ok(Self::Direct),
            MiscellaneousInput::Destination(Destination::Block) => Ok(Self::Block),
            MiscellaneousInput::Destination(Destination::Node { node_id }) => {
                Ok(Self::Node { node_id })
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ruleset {
    pub id: String,
    pub name: String,
    pub entries: Vec<RulesetEntry>,
    #[serde(default)]
    pub miscellaneous: Miscellaneous,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub mode: Mode,
    pub ruleset_id: Option<String>,
    pub global_node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub mode: Mode,
    pub ruleset_id: Option<String>,
    pub global_node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAppGroup {
    pub id: String,
    pub name: String,
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub services: Vec<ServiceView>,
    pub nodes: Vec<NodeView>,
    pub match_groups: Vec<MatchGroup>,
    pub rulesets: Vec<Ruleset>,
    pub presets: Vec<Preset>,
    pub selection: Selection,
    pub custom_app_groups: Vec<CustomAppGroup>,
    pub history_paused: bool,
}

fn default_schema_version() -> u32 {
    2
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            services: Vec::new(),
            nodes: Vec::new(),
            match_groups: Vec::new(),
            rulesets: Vec::new(),
            presets: Vec::new(),
            selection: Selection::default(),
            custom_app_groups: Vec::new(),
            history_paused: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreStatus {
    Off,
    Starting,
    On,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeView {
    pub vpn_active: bool,
    pub core_status: CoreStatus,
    pub service_available: bool,
    pub service_message: Option<String>,
    pub last_error: Option<String>,
    pub active_summary: Option<String>,
    #[serde(default)]
    pub competing_tunnel: bool,
    #[serde(default)]
    pub pending_reconnect_services: Vec<String>,
}

impl Default for RuntimeView {
    fn default() -> Self {
        Self {
            vpn_active: false,
            core_status: CoreStatus::Off,
            service_available: false,
            service_message: Some(
                "The Clashfork service is not installed. Reinstall Clashfork to enable the TUN VPN."
                    .to_owned(),
            ),
            last_error: None,
            active_summary: None,
            competing_tunnel: false,
            pending_reconnect_services: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub state: AppState,
    pub runtime: RuntimeView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewNode {
    pub display_name: String,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub id: String,
    pub source_type: SourceType,
    pub title: String,
    pub source_host: Option<String>,
    pub node_count: usize,
    pub unsupported_count: usize,
    pub notice_count: usize,
    pub allowance: Allowance,
    pub stale: bool,
    pub nodes: Vec<PreviewNode>,
}

#[derive(Clone)]
pub struct PendingImport {
    pub id: String,
    pub title: String,
    pub source_type: SourceType,
    pub source_url: Option<String>,
    pub source_host: Option<String>,
    pub nodes: Vec<ParsedNode>,
    pub unsupported_count: usize,
    pub notice_count: usize,
    pub allowance: Allowance,
}

#[derive(Clone)]
pub struct ParsedNode {
    pub source_id: String,
    pub display_name: String,
    pub protocol: String,
    pub server: String,
    pub port: u16,
    pub secret_material: String,
    pub proxy: Value,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub token: String,
    pub generation: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredNodeLatency {
    pub definition_fingerprint: String,
    pub latency_ms: u64,
    pub measured_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTestProgress {
    pub total: usize,
    pub completed: usize,
    pub responded: usize,
    pub failed: usize,
    pub node_id: Option<String>,
    pub latency_ms: Option<u64>,
    pub finished: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTestSummary {
    pub total: usize,
    pub completed: usize,
    pub responded: usize,
    pub failed: usize,
    pub cancelled: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SecretStore {
    #[serde(default)]
    pub subscriptions: BTreeMap<String, String>,
    #[serde(default)]
    pub nodes: BTreeMap<String, Value>,
    #[serde(default)]
    pub node_latencies: BTreeMap<String, StoredNodeLatency>,
    #[serde(default)]
    pub last_session: Option<StoredSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveRoute {
    pub app: String,
    pub app_key: String,
    pub destination: String,
    pub network: String,
    pub route: String,
    pub matched_group: Option<String>,
    pub download_rate: u64,
    pub upload_rate: u64,
    pub download_total: u64,
    pub upload_total: u64,
    pub started_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    pub active: bool,
    pub available: bool,
    pub download_rate: u64,
    pub upload_rate: u64,
    pub download_total: u64,
    pub upload_total: u64,
    pub connections: Vec<LiveRoute>,
    pub sampled_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LiveRates {
    pub active: bool,
    pub available: bool,
    pub proxy_download_rate: u64,
    pub proxy_upload_rate: u64,
    pub sampled_at: i64,
}

#[cfg(test)]
mod live_rates_tests {
    use super::LiveRates;

    #[test]
    fn live_rates_payload_contains_no_connection_rows() {
        let payload = serde_json::to_value(LiveRates::default()).unwrap();
        assert_eq!(
            payload
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "active",
                "available",
                "proxyDownloadRate",
                "proxyUploadRate",
                "sampledAt",
            ]
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryBar {
    pub start: i64,
    pub label: String,
    pub download: u64,
    pub upload: u64,
    #[serde(default)]
    pub proxy_total: u64,
    #[serde(default)]
    pub direct_total: u64,
    pub gap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RouteClass {
    Proxy,
    Direct,
    Blocked,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRow {
    pub label: String,
    pub app: Option<String>,
    pub route: Option<String>,
    pub route_class: RouteClass,
    pub download: u64,
    pub upload: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryReport {
    pub from: i64,
    pub to: i64,
    pub total_download: u64,
    pub total_upload: u64,
    pub total: u64,
    pub gap_minutes: u64,
    pub bars: Vec<HistoryBar>,
    pub rows: Vec<HistoryRow>,
    pub breakdown: String,
    pub available_apps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEntry {
    pub id: u64,
    pub at: i64,
    pub source: String,
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddNodesInput {
    pub value: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRulesetInput {
    pub id: Option<String>,
    pub name: String,
    pub entries: Vec<RulesetEntry>,
    pub miscellaneous: Miscellaneous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveMatchGroupInput {
    pub id: Option<String>,
    pub name: String,
    pub conditions: Vec<ConditionDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionDraft {
    pub kind: ConditionKind,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryInput {
    pub range: String,
    pub breakdown: String,
    #[serde(default)]
    pub route_filter: Option<HistoryRouteFilter>,
    #[serde(default)]
    pub subject_filter: Option<HistorySubjectFilter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryRouteFilter {
    Proxy,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HistorySubjectFilter {
    Group { id: String },
    App { name: String },
}
