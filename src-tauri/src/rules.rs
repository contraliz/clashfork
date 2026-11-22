use crate::models::{
    AppState, ConditionKind, Destination, MatchCondition, MatchGroup, Mode, NodeView, Ruleset,
    SecretStore, Selection,
};
use anyhow::{Context, Result, bail};
use serde_yaml_ng::{Mapping, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

pub const APP_GLOBAL_GROUP: &str = "GLOBAL";
pub const TUN_DEVICE: &str = "ClashforkTUN";
pub const CORE_CONTROLLER_PORT: u16 = 19197;
pub const DNS_PORT: u16 = 19198;

/// Resolvers that do not depend on the tunnel or a system proxy. DIRECT traffic and proxy
/// bootstrap both use this pool so a single unreachable DoH endpoint cannot stall DNS.
const UNPROXIED_DOH: &[&str] = &["https://1.1.1.1/dns-query", "https://223.5.5.5/dns-query"];

#[derive(Debug, Clone)]
pub struct ControllerConfig {
    pub port: u16,
    pub secret: String,
    pub dns_port: u16,
}

#[derive(Debug, Clone)]
pub struct CompiledRuntime {
    pub yaml: String,
    pub node_routes: BTreeMap<String, String>,
    pub rule_groups: BTreeMap<String, String>,
}

pub fn generate_id(prefix: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &id[..24])
}

pub fn validate_group_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("Group names cannot be empty.");
    }
    if trimmed.chars().count() > 80 {
        bail!("Group names must be 80 characters or fewer.");
    }
    if trimmed.chars().any(|c| c.is_control() || c == ',') {
        bail!("Group names cannot contain commas or control characters.");
    }
    Ok(trimmed.to_owned())
}

pub fn validate_ruleset_name(name: &str) -> Result<String> {
    validate_group_name(name).context("Invalid ruleset name")
}

pub fn validate_condition(condition: &MatchCondition) -> Result<String> {
    validate_condition_value(condition.kind, &condition.value)
}

pub fn validate_condition_value(kind: ConditionKind, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("Condition values cannot be empty.");
    }
    if trimmed.chars().any(|c| c.is_control() || c == ',') {
        bail!("Condition values cannot contain commas or control characters.");
    }
    if trimmed.chars().count() > 512 {
        bail!("Condition values must be 512 characters or fewer.");
    }
    match kind {
        ConditionKind::ProcessName => {
            if trimmed.contains(['/', '\\', ':']) {
                bail!("Process names must be a basename, without a path.");
            }
            Ok(trimmed.to_owned())
        }
        ConditionKind::ProcessPath => {
            let is_absolute = trimmed.starts_with(r"\\")
                || trimmed.as_bytes().get(1).is_some_and(|b| *b == b':')
                || trimmed.starts_with('/');
            if !is_absolute {
                bail!("Process paths must be absolute.");
            }
            Ok(trimmed.to_owned())
        }
        ConditionKind::Domain | ConditionKind::DomainSuffix => {
            let normalized = trimmed
                .trim_start_matches('.')
                .trim_end_matches('.')
                .to_ascii_lowercase();
            if trimmed.contains("://") || normalized.contains(['/', ':']) {
                bail!("Domains must not contain a scheme, port, or path.");
            }
            validate_domain(&normalized)?;
            Ok(normalized)
        }
        ConditionKind::Ipv4Cidr => validate_cidr_v4(trimmed),
        ConditionKind::Ipv6Cidr => validate_cidr_v6(trimmed),
    }
}

fn validate_domain(domain: &str) -> Result<()> {
    if domain.is_empty() || domain.len() > 253 {
        bail!("Invalid domain.");
    }
    for label in domain.split('.') {
        if label.is_empty() || label.len() > 63 {
            bail!("Invalid domain.");
        }
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            bail!("Domains may contain only letters, digits, dots, underscores, and hyphens.");
        }
        if label.starts_with('-') || label.ends_with('-') {
            bail!("Domain labels cannot begin or end with a hyphen.");
        }
    }
    Ok(())
}

fn validate_cidr_v4(value: &str) -> Result<String> {
    let (address, prefix) = value
        .split_once('/')
        .context("IPv4 CIDR must include a prefix.")?;
    let address = Ipv4Addr::from_str(address).context("Invalid IPv4 address.")?;
    let prefix: u8 = prefix.parse().context("Invalid IPv4 CIDR prefix.")?;
    if prefix > 32 {
        bail!("IPv4 CIDR prefixes must be between 0 and 32.");
    }
    Ok(format!("{address}/{prefix}"))
}

fn validate_cidr_v6(value: &str) -> Result<String> {
    let (address, prefix) = value
        .split_once('/')
        .context("IPv6 CIDR must include a prefix.")?;
    let address = Ipv6Addr::from_str(address).context("Invalid IPv6 address.")?;
    let prefix: u8 = prefix.parse().context("Invalid IPv6 CIDR prefix.")?;
    if prefix > 128 {
        bail!("IPv6 CIDR prefixes must be between 0 and 128.");
    }
    Ok(format!("{address}/{prefix}"))
}

pub fn validate_match_group(group: &MatchGroup) -> Result<()> {
    validate_group_name(&group.name)?;
    if group.conditions.is_empty() {
        bail!("Match groups need at least one condition.");
    }
    let mut seen = BTreeSet::new();
    for condition in &group.conditions {
        let value = validate_condition(condition)?;
        let key = format!("{:?}:{}", condition.kind, value.to_ascii_lowercase());
        if !seen.insert(key) {
            bail!("Match groups cannot contain the same condition twice.");
        }
    }
    Ok(())
}

pub fn validate_ruleset(ruleset: &Ruleset, state: &AppState) -> Result<()> {
    validate_ruleset_name(&ruleset.name)?;
    let mut seen_groups = BTreeSet::new();
    for entry in &ruleset.entries {
        if !seen_groups.insert(entry.match_group_id.clone()) {
            bail!("A match group can appear only once in a ruleset.");
        }
        let group = state
            .match_groups
            .iter()
            .find(|group| group.id == entry.match_group_id)
            .context("A ruleset entry references a missing match group.")?;
        validate_match_group(group)?;
        if let Destination::Node { node_id } = &entry.destination {
            validate_node_reference(state, node_id)?;
        }
    }
    Ok(())
}

pub fn validate_node_reference<'a>(state: &'a AppState, node_id: &str) -> Result<&'a NodeView> {
    let node = state
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .context("A route references a node that no longer exists.")?;
    if node.missing {
        bail!(
            "The node “{}” is missing from its service. Choose a replacement before activating.",
            node.display_name
        );
    }
    Ok(node)
}

pub fn selection_is_valid(state: &AppState, selection: &Selection) -> Result<()> {
    match selection.mode {
        Mode::Rule => {
            let ruleset = active_ruleset(state, selection)?;
            validate_ruleset(ruleset, state)?;
        }
        Mode::Global => {
            let node_id = selection
                .global_node_id
                .as_deref()
                .context("Select a node before enabling Global mode.")?;
            validate_node_reference(state, node_id)?;
        }
    }
    Ok(())
}

pub fn active_ruleset<'a>(state: &'a AppState, selection: &Selection) -> Result<&'a Ruleset> {
    let id = selection
        .ruleset_id
        .as_deref()
        .context("Create and select a ruleset before enabling Rule mode.")?;
    state
        .rulesets
        .iter()
        .find(|ruleset| ruleset.id == id)
        .context("The selected ruleset no longer exists.")
}

pub fn active_summary(state: &AppState, selection: &Selection) -> Result<String> {
    match selection.mode {
        Mode::Rule => {
            let ruleset = active_ruleset(state, selection)?;
            Ok(format!("Rule · {}", ruleset.name))
        }
        Mode::Global => {
            let node_id = selection
                .global_node_id
                .as_deref()
                .context("Select a node before enabling Global mode.")?;
            let node = validate_node_reference(state, node_id)?;
            Ok(format!(
                "Global · {} / {}",
                node.service_title, node.display_name
            ))
        }
    }
}

fn matcher(kind: ConditionKind, value: &str) -> String {
    match kind {
        ConditionKind::ProcessName => format!("PROCESS-NAME,{value}"),
        ConditionKind::ProcessPath => format!("PROCESS-PATH,{value}"),
        ConditionKind::Domain => format!("DOMAIN,{value}"),
        ConditionKind::DomainSuffix => format!("DOMAIN-SUFFIX,{value}"),
        ConditionKind::Ipv4Cidr => format!("IP-CIDR,{value},no-resolve"),
        ConditionKind::Ipv6Cidr => format!("IP-CIDR6,{value},no-resolve"),
    }
}

/// A node match is immediately followed by the same matcher rejecting. Stock Mihomo continues
/// after a node that cannot carry UDP; the reject is reached only on that fall-through path.
fn compile_conditions(
    conditions: &[MatchCondition],
    target: &str,
    node_target: bool,
    rules: &mut Vec<String>,
) -> Result<()> {
    for condition in conditions {
        let value = validate_condition(condition)?;
        let base = matcher(condition.kind, &value);
        rules.push(format!("{base},{target}"));
        if node_target {
            rules.push(format!("{base},REJECT"));
        }
    }
    Ok(())
}

pub fn compile_rule_lines(state: &AppState, selection: &Selection) -> Result<Vec<String>> {
    match selection.mode {
        Mode::Global => Ok(Vec::new()),
        Mode::Rule => {
            let ruleset = active_ruleset(state, selection)?;
            validate_ruleset(ruleset, state)?;
            let groups: BTreeMap<&str, &MatchGroup> = state
                .match_groups
                .iter()
                .map(|group| (group.id.as_str(), group))
                .collect();
            let nodes: BTreeMap<&str, &NodeView> = state
                .nodes
                .iter()
                .map(|node| (node.id.as_str(), node))
                .collect();
            let mut rules = Vec::new();
            for entry in &ruleset.entries {
                let group = groups
                    .get(entry.match_group_id.as_str())
                    .context("A ruleset references a missing match group.")?;
                match &entry.destination {
                    Destination::Direct => {
                        compile_conditions(&group.conditions, "DIRECT", false, &mut rules)?;
                    }
                    Destination::Block => {
                        compile_conditions(&group.conditions, "REJECT", false, &mut rules)?;
                    }
                    Destination::Node { node_id } => {
                        let node = nodes
                            .get(node_id.as_str())
                            .context("A ruleset references a missing node.")?;
                        if node.missing {
                            bail!("A ruleset references a node missing from its service.");
                        }
                        compile_conditions(&group.conditions, &node.core_name, true, &mut rules)?;
                    }
                }
            }
            rules.push(format!(
                "MATCH,{}",
                match ruleset.miscellaneous {
                    crate::models::Miscellaneous::Direct => "DIRECT",
                    crate::models::Miscellaneous::Block => "REJECT",
                }
            ));
            Ok(rules)
        }
    }
}

fn required_str<'a>(mapping: &'a Mapping, key: &str) -> Result<&'a str> {
    mapping
        .get(Value::String(key.to_owned()))
        .and_then(Value::as_str)
        .with_context(|| format!("A stored node is missing its {key} field."))
}

fn map_insert(mapping: &mut Mapping, key: &str, value: Value) {
    mapping.insert(Value::String(key.to_owned()), value);
}

fn sequence_strings(values: &[&str]) -> Value {
    Value::Sequence(
        values
            .iter()
            .map(|value| Value::String((*value).to_owned()))
            .collect(),
    )
}

pub fn build_runtime_yaml(
    state: &AppState,
    secrets: &SecretStore,
    selection: &Selection,
    controller: &ControllerConfig,
    tun_enabled: bool,
) -> Result<CompiledRuntime> {
    selection_is_valid(state, selection)?;

    let mut proxies = Vec::new();
    let mut node_routes = BTreeMap::new();
    for node in state.nodes.iter().filter(|node| !node.missing) {
        let proxy = secrets.nodes.get(&node.id).with_context(|| {
            format!(
                "The saved credential for “{}” is unavailable. Refresh or re-add its service.",
                node.display_name
            )
        })?;
        let mut proxy = proxy
            .as_mapping()
            .cloned()
            .context("A stored node definition is invalid.")?;
        let kind = required_str(&proxy, "type")?.to_ascii_lowercase();
        if !matches!(kind.as_str(), "trojan" | "ss" | "vmess" | "vless") {
            continue;
        }
        map_insert(&mut proxy, "name", Value::String(node.core_name.clone()));
        proxies.push(Value::Mapping(proxy));
        node_routes.insert(
            node.core_name.clone(),
            format!("{} / {}", node.service_title, node.display_name),
        );
    }

    let mut proxy_groups = Vec::new();
    let mode = match selection.mode {
        Mode::Rule => "rule",
        Mode::Global => {
            let node_id = selection
                .global_node_id
                .as_deref()
                .context("Select a node before enabling Global mode.")?;
            validate_node_reference(state, node_id)?;
            let node = state
                .nodes
                .iter()
                .find(|node| node.id == node_id && !node.missing)
                .context("The selected Global node is unavailable.")?;
            if !secrets.nodes.contains_key(node_id) {
                bail!("The selected Global node has no saved credentials.");
            }
            let mut group = Mapping::new();
            map_insert(
                &mut group,
                "name",
                Value::String(APP_GLOBAL_GROUP.to_owned()),
            );
            map_insert(&mut group, "type", Value::String("select".to_owned()));
            map_insert(
                &mut group,
                "proxies",
                Value::Sequence(vec![Value::String(node.core_name.clone())]),
            );
            proxy_groups.push(Value::Mapping(group));
            "global"
        }
    };

    let rules = compile_rule_lines(state, selection)?;
    let rule_groups = if selection.mode == Mode::Rule {
        let ruleset = active_ruleset(state, selection)?;
        ruleset
            .entries
            .iter()
            .filter_map(|entry| match &entry.destination {
                Destination::Node { node_id } => state
                    .nodes
                    .iter()
                    .find(|node| &node.id == node_id)
                    .map(|node| (node.core_name.clone(), entry.match_group_id.clone())),
                _ => None,
            })
            .collect()
    } else {
        BTreeMap::new()
    };

    let mut root = Mapping::new();
    map_insert(&mut root, "mixed-port", Value::from(0));
    map_insert(&mut root, "allow-lan", Value::Bool(false));
    map_insert(&mut root, "mode", Value::String(mode.to_owned()));
    map_insert(&mut root, "log-level", Value::String("info".to_owned()));
    map_insert(&mut root, "ipv6", Value::Bool(true));
    map_insert(
        &mut root,
        "find-process-mode",
        Value::String("always".to_owned()),
    );
    map_insert(&mut root, "unified-delay", Value::Bool(true));
    map_insert(
        &mut root,
        "external-controller",
        Value::String(format!("127.0.0.1:{}", controller.port)),
    );
    map_insert(
        &mut root,
        "secret",
        Value::String(controller.secret.clone()),
    );
    let mut cors = Mapping::new();
    map_insert(&mut cors, "allow-private-network", Value::Bool(false));
    map_insert(&mut cors, "allow-origins", Value::Sequence(Vec::new()));
    map_insert(&mut root, "external-controller-cors", Value::Mapping(cors));
    let mut profile = Mapping::new();
    map_insert(&mut profile, "store-selected", Value::Bool(false));
    map_insert(&mut profile, "store-fake-ip", Value::Bool(false));
    map_insert(&mut root, "profile", Value::Mapping(profile));

    let mut tun = Mapping::new();
    map_insert(&mut tun, "enable", Value::Bool(tun_enabled));
    map_insert(&mut tun, "stack", Value::String("mixed".to_owned()));
    map_insert(&mut tun, "device", Value::String(TUN_DEVICE.to_owned()));
    map_insert(&mut tun, "auto-route", Value::Bool(true));
    map_insert(&mut tun, "auto-detect-interface", Value::Bool(true));
    map_insert(&mut tun, "strict-route", Value::Bool(true));
    map_insert(
        &mut tun,
        "dns-hijack",
        sequence_strings(&["any:53", "tcp://any:53"]),
    );
    map_insert(
        &mut tun,
        "route-exclude-address",
        sequence_strings(&[
            "10.0.0.0/8",
            "127.0.0.0/8",
            "169.254.0.0/16",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "::1/128",
            "fc00::/7",
            "fe80::/10",
        ]),
    );
    map_insert(&mut tun, "mtu", Value::from(9000));
    map_insert(&mut root, "tun", Value::Mapping(tun));

    let mut dns = Mapping::new();
    map_insert(&mut dns, "enable", Value::Bool(true));
    map_insert(
        &mut dns,
        "listen",
        Value::String(format!("127.0.0.1:{}", controller.dns_port)),
    );
    map_insert(&mut dns, "ipv6", Value::Bool(true));
    map_insert(
        &mut dns,
        "enhanced-mode",
        Value::String("fake-ip".to_owned()),
    );
    map_insert(
        &mut dns,
        "fake-ip-range",
        Value::String("198.18.0.1/16".to_owned()),
    );
    map_insert(&mut dns, "respect-rules", Value::Bool(false));
    map_insert(&mut dns, "use-hosts", Value::Bool(true));
    map_insert(&mut dns, "use-system-hosts", Value::Bool(true));
    map_insert(
        &mut dns,
        "default-nameserver",
        sequence_strings(&["223.5.5.5", "1.1.1.1"]),
    );
    let nameservers: Vec<String> = if selection.mode == Mode::Global {
        UNPROXIED_DOH
            .iter()
            .map(|server| format!("{server}#GLOBAL"))
            .collect()
    } else {
        UNPROXIED_DOH
            .iter()
            .map(|server| (*server).to_owned())
            .collect()
    };
    map_insert(
        &mut dns,
        "nameserver",
        Value::Sequence(nameservers.into_iter().map(Value::String).collect()),
    );
    map_insert(
        &mut dns,
        "proxy-server-nameserver",
        sequence_strings(UNPROXIED_DOH),
    );
    map_insert(
        &mut dns,
        "direct-nameserver",
        sequence_strings(UNPROXIED_DOH),
    );
    map_insert(
        &mut dns,
        "fake-ip-filter",
        sequence_strings(&[
            "*.lan",
            "*.local",
            "+.local",
            "localhost.ptlogin2.qq.com",
            "+.msftconnecttest.com",
            "+.msftncsi.com",
            "time.*.com",
            "ntp.*.com",
        ]),
    );
    map_insert(&mut root, "dns", Value::Mapping(dns));

    map_insert(&mut root, "proxies", Value::Sequence(proxies));
    if !proxy_groups.is_empty() {
        map_insert(&mut root, "proxy-groups", Value::Sequence(proxy_groups));
    }
    map_insert(
        &mut root,
        "rules",
        Value::Sequence(rules.into_iter().map(Value::String).collect()),
    );

    let yaml = serde_yaml_ng::to_string(&Value::Mapping(root))
        .context("Could not serialize the routing configuration.")?;
    Ok(CompiledRuntime {
        yaml,
        node_routes,
        rule_groups,
    })
}

/// Config validation for the bundled stock core. TUN is retained; this only parses the
/// configuration and never creates a network interface.
pub fn validate_with_core(
    core: &std::path::Path,
    data_dir: &std::path::Path,
    yaml: &str,
) -> Result<()> {
    use std::process::Command;
    std::fs::create_dir_all(data_dir)?;
    let temp = crate::paths::private_temp_file(data_dir, "check", yaml.as_bytes())?;
    let output = Command::new(core)
        .arg("-t")
        .arg("-d")
        .arg(data_dir)
        .arg("-f")
        .arg(&temp)
        .output()
        .context("Could not start the bundled core for configuration validation.")?;
    let _ = std::fs::remove_file(&temp);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stdout}\n{stderr}");
        let safe = crate::diagnostics::redact(&combined);
        bail!("The routing configuration did not pass the stock core check: {safe}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn node(id: &str, core: &str) -> NodeView {
        NodeView {
            id: id.into(),
            service_id: "service".into(),
            service_title: "Falemon".into(),
            display_name: "Singapore I0".into(),
            protocol: "Trojan".into(),
            missing: false,
            core_name: core.into(),
        }
    }

    fn base() -> (AppState, SecretStore, Selection) {
        let state = AppState {
            schema_version: 1,
            services: vec![],
            nodes: vec![node("node-1", "cf_node_1"), node("node-2", "cf_node_2")],
            match_groups: vec![MatchGroup {
                id: "group-1".into(),
                name: "OpenAI".into(),
                conditions: vec![
                    MatchCondition {
                        id: "c1".into(),
                        kind: ConditionKind::ProcessName,
                        value: "ChatGPT.exe".into(),
                    },
                    MatchCondition {
                        id: "c2".into(),
                        kind: ConditionKind::ProcessName,
                        value: "codex.exe".into(),
                    },
                    MatchCondition {
                        id: "c3".into(),
                        kind: ConditionKind::DomainSuffix,
                        value: "chatgpt.com".into(),
                    },
                ],
            }],
            rulesets: vec![Ruleset {
                id: "ruleset-1".into(),
                name: "AI".into(),
                entries: vec![RulesetEntry {
                    id: "entry-1".into(),
                    match_group_id: "group-1".into(),
                    destination: Destination::Node {
                        node_id: "node-1".into(),
                    },
                }],
                miscellaneous: Miscellaneous::Direct,
            }],
            presets: vec![],
            selection: Selection {
                mode: Mode::Rule,
                ruleset_id: Some("ruleset-1".into()),
                global_node_id: None,
            },
            custom_app_groups: vec![],
            history_paused: false,
        };
        let mut secrets = SecretStore::default();
        let mut first = Mapping::new();
        map_insert(&mut first, "type", Value::String("trojan".into()));
        map_insert(
            &mut first,
            "server",
            Value::String("example.invalid".into()),
        );
        map_insert(&mut first, "port", Value::from(443));
        map_insert(
            &mut first,
            "password",
            Value::String("fixture-password".into()),
        );
        secrets
            .nodes
            .insert("node-1".into(), Value::Mapping(first.clone()));
        secrets.nodes.insert("node-2".into(), Value::Mapping(first));
        let selection = state.selection.clone();
        (state, secrets, selection)
    }

    #[test]
    fn compile_or_conditions_and_non_udp_guard() {
        let (state, _, selection) = base();
        let rules = compile_rule_lines(&state, &selection).unwrap();
        assert_eq!(
            rules,
            vec![
                "PROCESS-NAME,ChatGPT.exe,cf_node_1",
                "PROCESS-NAME,ChatGPT.exe,REJECT",
                "PROCESS-NAME,codex.exe,cf_node_1",
                "PROCESS-NAME,codex.exe,REJECT",
                "DOMAIN-SUFFIX,chatgpt.com,cf_node_1",
                "DOMAIN-SUFFIX,chatgpt.com,REJECT",
                "MATCH,DIRECT",
            ]
        );
    }

    #[test]
    fn group_order_is_preserved_and_misc_is_last() {
        let (mut state, _, selection) = base();
        state.match_groups.push(MatchGroup {
            id: "group-2".into(),
            name: "Work".into(),
            conditions: vec![MatchCondition {
                id: "c4".into(),
                kind: ConditionKind::Domain,
                value: "example.com".into(),
            }],
        });
        state.rulesets[0].entries.insert(
            0,
            RulesetEntry {
                id: "entry-2".into(),
                match_group_id: "group-2".into(),
                destination: Destination::Direct,
            },
        );
        let rules = compile_rule_lines(&state, &selection).unwrap();
        assert_eq!(rules[0], "DOMAIN,example.com,DIRECT");
        assert_eq!(rules.last().unwrap(), "MATCH,DIRECT");
    }

    #[test]
    fn empty_rulesets_accept_and_compile_explicit_miscellaneous_actions() {
        let (mut state, _, selection) = base();
        state.rulesets[0].entries.clear();

        for (miscellaneous, expected) in [
            (Miscellaneous::Direct, "MATCH,DIRECT"),
            (Miscellaneous::Block, "MATCH,REJECT"),
        ] {
            state.rulesets[0].miscellaneous = miscellaneous;
            selection_is_valid(&state, &selection).unwrap();
            assert_eq!(
                compile_rule_lines(&state, &selection).unwrap(),
                vec![expected]
            );
        }
    }

    #[test]
    fn global_pins_one_node_without_direct_choice() {
        let (mut state, secrets, _) = base();
        state.selection = Selection {
            mode: Mode::Global,
            ruleset_id: None,
            global_node_id: Some("node-2".into()),
        };
        let compiled = build_runtime_yaml(
            &state,
            &secrets,
            &state.selection,
            &ControllerConfig {
                port: 19000,
                secret: "fixture-secret".into(),
                dns_port: 19198,
            },
            true,
        )
        .unwrap();
        let value: Value = serde_yaml_ng::from_str(&compiled.yaml).unwrap();
        assert_eq!(value["mode"], Value::String("global".into()));
        let groups = value["proxy-groups"].as_sequence().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["name"], Value::String("GLOBAL".into()));
        assert_eq!(groups[0]["proxies"][0], Value::String("cf_node_2".into()));
        assert_eq!(
            value["dns"]["nameserver"][0],
            "https://1.1.1.1/dns-query#GLOBAL"
        );
        assert!(!compiled.yaml.contains("GLOBAL,DIRECT"));
        assert!(
            !groups[0]["proxies"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|v| v == "DIRECT")
        );
    }

    #[test]
    fn direct_dns_uses_the_shared_unproxied_resolver_pool() {
        let (state, secrets, rule) = base();
        let global = Selection {
            mode: Mode::Global,
            ruleset_id: None,
            global_node_id: Some("node-2".into()),
        };
        let controller = ControllerConfig {
            port: 19000,
            secret: "fixture-secret".into(),
            dns_port: 19198,
        };
        let expected_pool = vec![
            Value::String("https://1.1.1.1/dns-query".into()),
            Value::String("https://223.5.5.5/dns-query".into()),
        ];

        for (selection, global_mode) in [(rule, false), (global, true)] {
            let compiled =
                build_runtime_yaml(&state, &secrets, &selection, &controller, true).unwrap();
            let value: Value = serde_yaml_ng::from_str(&compiled.yaml).unwrap();
            let dns = &value["dns"];

            assert_eq!(
                dns["direct-nameserver"].as_sequence(),
                Some(&expected_pool),
                "DIRECT resolution must retain both unproxied DoH resolvers"
            );
            assert_eq!(
                dns["proxy-server-nameserver"].as_sequence(),
                Some(&expected_pool),
                "proxy bootstrap must remain independent of general DNS routing"
            );
            let nameservers = dns["nameserver"].as_sequence().unwrap();
            if global_mode {
                assert_eq!(
                    nameservers,
                    &vec![
                        Value::String("https://1.1.1.1/dns-query#GLOBAL".into()),
                        Value::String("https://223.5.5.5/dns-query#GLOBAL".into()),
                    ]
                );
            } else {
                assert_eq!(nameservers, &expected_pool);
            }
        }
    }

    #[test]
    #[ignore = "requires the bundled Mihomo executable"]
    fn stock_mihomo_dns_fails_over_to_a_local_fixture() {
        use std::{
            net::UdpSocket,
            process::{Child, Command, Stdio},
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
            time::Duration,
        };

        struct RunningCore(Child);
        impl Drop for RunningCore {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        fn query(id: u16, name: &str) -> Vec<u8> {
            let mut packet = Vec::with_capacity(64);
            packet.extend_from_slice(&id.to_be_bytes());
            packet.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
            for label in name.split('.') {
                packet.push(label.len() as u8);
                packet.extend_from_slice(label.as_bytes());
            }
            packet.push(0);
            packet.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]);
            packet
        }

        let fixture = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        fixture
            .set_read_timeout(Some(Duration::from_millis(250)))
            .unwrap();
        let fixture_port = fixture.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_server = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            let mut request = [0u8; 512];
            while !stop_server.load(Ordering::Relaxed) {
                let Ok((length, from)) = fixture.recv_from(&mut request) else {
                    continue;
                };
                let question_end = {
                    let mut index = 12;
                    while index < length && request[index] != 0 {
                        index += request[index] as usize + 1;
                    }
                    index + 5
                };
                if length < 16 || request[question_end - 4..question_end - 2] != [0, 1] {
                    continue;
                }
                let mut response = Vec::with_capacity(64);
                response.extend_from_slice(&request[0..2]);
                response.extend_from_slice(&[
                    0x81, 0x80, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
                ]);
                response.extend_from_slice(&request[12..question_end]);
                response.extend_from_slice(&[
                    0xc0, 0x0c, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 203, 0,
                    113, 7,
                ]);
                let _ = fixture.send_to(&response, from);
            }
        });

        let dead = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);
        let listener = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let dns_port = listener.local_addr().unwrap().port();
        drop(listener);

        let config = format!(
            "mixed-port: 0\nmode: direct\nipv6: false\ndns:\n  enable: true\n  listen: 127.0.0.1:{dns_port}\n  ipv6: false\n  enhanced-mode: normal\n  nameserver:\n    - 127.0.0.1:{dead_port}\n    - 127.0.0.1:{fixture_port}\nrules:\n  - MATCH,DIRECT\n"
        );
        let core =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/clashfork-core.exe");
        let scratch = tempfile::tempdir().unwrap();
        let config_path = scratch.path().join("dns-fixture.yaml");
        std::fs::write(&config_path, config).unwrap();
        let _child = RunningCore(
            Command::new(core)
                .arg("-d")
                .arg(scratch.path())
                .arg("-f")
                .arg(&config_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );

        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(250)))
            .unwrap();
        let mut resolved = false;
        for attempt in 0..30 {
            let _ = client.send_to(
                &query(0x1234 + attempt, "fixture.test"),
                ("127.0.0.1", dns_port),
            );
            let mut response = [0u8; 512];
            if let Ok((length, _)) = client.recv_from(&mut response) {
                if response[..length]
                    .windows(4)
                    .any(|address| address == [203, 0, 113, 7])
                {
                    resolved = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        stop.store(true, Ordering::Relaxed);
        let _ = server.join();
        assert!(
            resolved,
            "the stock core did not fall back to the live local DNS fixture"
        );
    }

    #[test]
    #[ignore = "requires the bundled Mihomo executable"]
    fn stock_mihomo_validates_rule_and_global_configs() {
        let (state, secrets, rule) = base();
        let global = Selection {
            mode: Mode::Global,
            ruleset_id: None,
            global_node_id: Some("node-2".into()),
        };
        let controller = ControllerConfig {
            port: 19000,
            secret: "fixture-secret".into(),
            dns_port: 19198,
        };
        let core =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/clashfork-core.exe");
        let scratch = tempfile::tempdir().unwrap();
        for selection in [rule, global] {
            let config =
                build_runtime_yaml(&state, &secrets, &selection, &controller, true).unwrap();
            validate_with_core(&core, scratch.path(), &config.yaml).unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "requires the bundled Mihomo executable"]
    async fn stock_mihomo_reloads_a_rule_change_without_restarting() {
        use std::{
            net::TcpListener,
            path::Path,
            process::{Child, Command, Stdio},
        };

        struct RunningCore(Child);
        impl Drop for RunningCore {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let dns = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let config = ControllerConfig {
            port: listener.local_addr().unwrap().port(),
            secret: "clashfork-test-controller".into(),
            dns_port: dns.local_addr().unwrap().port(),
        };
        drop(listener);
        drop(dns);

        let (mut state, secrets, selection) = base();
        let before = build_runtime_yaml(&state, &secrets, &selection, &config, false).unwrap();
        let mut changed_ruleset = state.rulesets[0].clone();
        changed_ruleset.id = "ruleset-2".into();
        changed_ruleset.miscellaneous = Miscellaneous::Block;
        state.rulesets.push(changed_ruleset);
        let next = Selection {
            ruleset_id: Some("ruleset-2".into()),
            ..selection
        };
        let after = build_runtime_yaml(&state, &secrets, &next, &config, false).unwrap();

        let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/clashfork-core.exe");
        let scratch = tempfile::tempdir().unwrap();
        let initial_file = scratch.path().join("initial.yaml");
        let next_file = scratch.path().join("changed.yaml");
        std::fs::write(&initial_file, before.yaml).unwrap();
        std::fs::write(&next_file, after.yaml).unwrap();
        let mut child = RunningCore(
            Command::new(core)
                .arg("-d")
                .arg(scratch.path())
                .arg("-f")
                .arg(initial_file)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let controller = crate::controller::CoreController::new(
            config.port,
            config.secret.clone(),
            client.clone(),
        );
        crate::service_client::wait_ready(&controller)
            .await
            .unwrap();

        let last_target = || async {
            let response = client
                .get(format!("http://127.0.0.1:{}/rules", config.port))
                .bearer_auth(&config.secret)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap();
            response["rules"].as_array().unwrap().last().unwrap()["proxy"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(last_target().await, "DIRECT");
        controller
            .reload(next_file.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(last_target().await, "REJECT");

        let bad_file = scratch.path().join("bad.yaml");
        std::fs::write(&bad_file, "not: [valid").unwrap();
        assert!(controller.reload(bad_file.to_str().unwrap()).await.is_err());
        assert_eq!(
            last_target().await,
            "REJECT",
            "a rejected reload must leave the running rules unchanged"
        );

        let mut global_state = state.clone();
        global_state.selection = Selection {
            mode: Mode::Global,
            ruleset_id: None,
            global_node_id: Some("node-2".into()),
        };
        let global = build_runtime_yaml(
            &global_state,
            &secrets,
            &global_state.selection,
            &config,
            false,
        )
        .unwrap();
        let global_file = scratch.path().join("global.yaml");
        std::fs::write(&global_file, global.yaml).unwrap();
        controller
            .reload(global_file.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(controller.config_mode().await.unwrap(), "global");
        assert!(child.0.try_wait().unwrap().is_none());

        controller.close_connections().await.unwrap();
    }

    #[test]
    fn special_characters_are_rejected_before_reaching_the_core() {
        for (kind, value) in [
            (ConditionKind::Domain, "chatgpt.com\nMATCH,DIRECT"),
            (ConditionKind::ProcessName, "a,b"),
            (ConditionKind::DomainSuffix, "https://example.com"),
            (ConditionKind::ProcessPath, "relative.exe"),
            (ConditionKind::Ipv4Cidr, "999.1.1.1/24"),
            (ConditionKind::Ipv6Cidr, "::1/999"),
        ] {
            assert!(
                validate_condition_value(kind, value).is_err(),
                "{kind:?} {value:?} should be rejected"
            );
        }
    }

    #[test]
    fn missing_node_blocks_activation_and_is_named_safely() {
        let (mut state, _, selection) = base();
        state.nodes[0].missing = true;
        let error = selection_is_valid(&state, &selection)
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing"), "{error}");
        assert!(error.contains("Singapore I0"));
    }
}
