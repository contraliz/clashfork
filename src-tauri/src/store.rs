use crate::{
    models::{AppState, SecretStore},
    paths::AppPaths,
    security,
};
use anyhow::{Context, Result};
use std::path::Path;

pub fn load_state(paths: &AppPaths) -> Result<AppState> {
    if !paths.state_file.is_file() {
        return Ok(AppState::default());
    }
    let bytes = std::fs::read(&paths.state_file)
        .with_context(|| format!("failed to read {:?}", paths.state_file))?;
    let mut state: AppState =
        serde_json::from_slice(&bytes).context("failed to parse local application state")?;
    if state.schema_version == 0 {
        state.schema_version = 1;
    }
    if state.schema_version > 2 {
        anyhow::bail!("unsupported local state schema");
    }
    // Schema 2 makes each rule own its conditions. Copy legacy shared groups into
    // every referenced rule while retaining all groups for later condition copying.
    if state.schema_version < 2 {
        for ruleset in &mut state.rulesets {
            for entry in &mut ruleset.entries {
                if entry.conditions.is_empty() {
                    if let Some(group) = state
                        .match_groups
                        .iter()
                        .find(|g| g.id == entry.match_group_id)
                    {
                        entry.name = group.name.clone();
                        entry.conditions = group.conditions.clone();
                        for condition in &mut entry.conditions {
                            condition.id = format!("{}:{}", entry.id, condition.id);
                        }
                    }
                }
            }
        }
        state.schema_version = 2;
    }
    repair_references(&mut state);
    Ok(state)
}

pub fn save_state(paths: &AppPaths, state: &AppState) -> Result<()> {
    let temp = paths.state_file.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(state).context("failed to serialize local state")?;
    std::fs::write(&temp, bytes).context("failed to write local state")?;
    replace_file(&temp, &paths.state_file)
}

pub fn load_secrets(paths: &AppPaths) -> Result<SecretStore> {
    if !paths.secrets_file.is_file() {
        return Ok(SecretStore::default());
    }
    let plaintext = security::read_secrets_file(&paths.secrets_file)?;
    serde_json::from_slice(&plaintext).context("failed to parse protected local credentials")
}

pub fn save_secrets(paths: &AppPaths, secrets: &SecretStore) -> Result<()> {
    let plaintext =
        serde_json::to_vec(secrets).context("failed to serialize protected local credentials")?;
    security::write_secrets_file(&paths.secrets_file, &plaintext)?;
    Ok(())
}

fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        std::fs::remove_file(destination)
            .with_context(|| format!("failed to replace {:?}", destination))?;
    }
    std::fs::rename(source, destination)
        .with_context(|| format!("failed to publish {:?}", destination))?;
    Ok(())
}

fn repair_references(state: &mut AppState) {
    let service_titles: std::collections::BTreeMap<_, _> = state
        .services
        .iter()
        .map(|service| (service.id.clone(), service.title.clone()))
        .collect();
    state
        .nodes
        .retain(|node| service_titles.contains_key(&node.service_id));
    for node in &mut state.nodes {
        if let Some(title) = service_titles.get(&node.service_id) {
            node.service_title = title.clone();
        }
    }
    if state
        .selection
        .ruleset_id
        .as_ref()
        .is_some_and(|id| !state.rulesets.iter().any(|ruleset| &ruleset.id == id))
    {
        state.selection.ruleset_id = None;
    }
    if state
        .selection
        .global_node_id
        .as_ref()
        .is_some_and(|id| !state.nodes.iter().any(|node| &node.id == id))
    {
        state.selection.global_node_id = None;
    }
    state.presets.retain(|preset| {
        preset
            .ruleset_id
            .as_ref()
            .is_none_or(|id| state.rulesets.iter().any(|ruleset| &ruleset.id == id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Miscellaneous, Ruleset, Selection};
    use tempfile::tempdir;

    #[test]
    fn state_round_trips() {
        let dir = tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("appdata")).unwrap();
        let state = AppState::default();
        save_state(&paths, &state).unwrap();
        let loaded = load_state(&paths).unwrap();
        assert_eq!(loaded.schema_version, 2);
        assert!(loaded.services.is_empty());
    }

    #[test]
    fn empty_block_ruleset_and_dashboard_selection_round_trip() {
        let dir = tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("appdata")).unwrap();
        let mut state = AppState::default();
        state.rulesets.push(Ruleset {
            id: "ruleset-empty".into(),
            name: "Block by default".into(),
            entries: Vec::new(),
            miscellaneous: Miscellaneous::Block,
        });
        state.selection = Selection {
            mode: crate::models::Mode::Rule,
            ruleset_id: Some("ruleset-empty".into()),
            global_node_id: None,
        };

        save_state(&paths, &state).unwrap();
        let loaded = load_state(&paths).unwrap();

        assert_eq!(loaded.rulesets.len(), 1);
        assert!(loaded.rulesets[0].entries.is_empty());
        assert_eq!(loaded.rulesets[0].miscellaneous, Miscellaneous::Block);
        assert_eq!(
            loaded.selection.ruleset_id.as_deref(),
            Some("ruleset-empty")
        );
        let value = serde_json::to_value(&loaded.rulesets[0]).unwrap();
        assert_eq!(value["miscellaneous"], serde_json::json!({"kind":"block"}));
        assert_eq!(
            serde_json::from_value::<Miscellaneous>(serde_json::json!("block")).unwrap(),
            Miscellaneous::Block
        );
        assert_eq!(value["entries"], serde_json::json!([]));
    }

    #[test]
    fn schema_one_migration_copies_shared_conditions_and_keeps_unused_groups_and_routes() {
        use crate::models::{ConditionKind, Destination, MatchCondition, MatchGroup, RulesetEntry};
        let dir = tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("appdata")).unwrap();
        let mut state = AppState::default();
        state.schema_version = 1;
        state.nodes.push(crate::models::NodeView {
            id: "node-a".into(),
            service_id: "svc".into(),
            service_title: "Service".into(),
            display_name: "Node A".into(),
            protocol: "ss".into(),
            missing: false,
            omitted_once: false,
            core_name: "Node-A".into(),
            latency_ms: None,
            latency_measured_at_ms: None,
        });
        state.services.push(crate::models::ServiceView {
            id: "svc".into(),
            title: "Service".into(),
            source_type: crate::models::SourceType::Manual,
            source_host: None,
            node_count: 1,
            missing_count: 0,
            allowance: crate::models::Allowance::Unavailable,
            stale: false,
            omitted_node_ids: vec![],
            last_refresh_at: None,
            last_error: None,
        });
        state.match_groups = vec![
            MatchGroup {
                id: "shared".into(),
                name: "Shared label".into(),
                conditions: vec![MatchCondition {
                    id: "c1".into(),
                    kind: ConditionKind::DomainSuffix,
                    value: "example.org".into(),
                }],
            },
            MatchGroup {
                id: "unused".into(),
                name: "Unused source".into(),
                conditions: vec![MatchCondition {
                    id: "c2".into(),
                    kind: ConditionKind::ProcessName,
                    value: "app.exe".into(),
                }],
            },
        ];
        state.rulesets.push(crate::models::Ruleset {
            id: "r1".into(),
            name: "Kept".into(),
            entries: vec![
                RulesetEntry {
                    id: "e1".into(),
                    match_group_id: "shared".into(),
                    name: String::new(),
                    conditions: vec![],
                    destination: Destination::Node {
                        node_id: "node-a".into(),
                    },
                },
                RulesetEntry {
                    id: "e2".into(),
                    match_group_id: "shared".into(),
                    name: String::new(),
                    conditions: vec![],
                    destination: Destination::Block,
                },
            ],
            miscellaneous: crate::models::Miscellaneous::Block,
        });
        state.selection.ruleset_id = Some("r1".into());
        let mut value = serde_json::to_value(state).unwrap();
        value["schemaVersion"] = serde_json::json!(1);
        std::fs::write(&paths.state_file, serde_json::to_vec(&value).unwrap()).unwrap();
        let loaded = load_state(&paths).unwrap();
        assert_eq!(loaded.schema_version, 2);
        assert_eq!(loaded.rulesets[0].entries[0].name, "Shared label");
        assert_eq!(
            loaded.rulesets[0].entries[0].conditions[0].value,
            "example.org"
        );
        assert_ne!(
            loaded.rulesets[0].entries[0].conditions[0].id,
            loaded.rulesets[0].entries[1].conditions[0].id
        );
        assert!(
            matches!(loaded.rulesets[0].entries[0].destination, Destination::Node { ref node_id } if node_id == "node-a")
        );
        assert!(matches!(
            loaded.rulesets[0].entries[1].destination,
            Destination::Block
        ));
        assert_eq!(
            loaded.rulesets[0].miscellaneous,
            crate::models::Miscellaneous::Block
        );
        assert_eq!(loaded.selection.ruleset_id.as_deref(), Some("r1"));
        assert_eq!(loaded.match_groups.len(), 2);
        assert_eq!(loaded.match_groups[1].id, "unused");
        let compiled = crate::rules::compile_rule_lines(&loaded, &loaded.selection).unwrap();
        assert_eq!(compiled[0], "DOMAIN-SUFFIX,example.org,Node-A");
        assert_eq!(compiled[1], "DOMAIN-SUFFIX,example.org,REJECT");
        assert_eq!(compiled[2], "DOMAIN-SUFFIX,example.org,REJECT");
        assert_eq!(compiled.last().unwrap(), "MATCH,REJECT");
    }

    #[test]
    fn malformed_state_is_never_rewritten_by_load_or_migration() {
        let dir = tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("appdata")).unwrap();
        let malformed = br#"{"schemaVersion":1,"rulesets":["#;
        std::fs::write(&paths.state_file, malformed).unwrap();
        assert!(load_state(&paths).is_err());
        assert_eq!(std::fs::read(&paths.state_file).unwrap(), malformed);
    }

    #[test]
    fn protected_secrets_never_contain_plaintext_on_disk() {
        let dir = tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("appdata")).unwrap();
        let mut secrets = SecretStore::default();
        secrets.subscriptions.insert(
            "svc".into(),
            "https://fixture.invalid/sub/private-path-token".into(),
        );
        secrets.nodes.insert(
            "node".into(),
            serde_yaml_ng::from_str("password: fixture-private-password\n").unwrap(),
        );
        secrets.node_latencies.insert(
            "node".into(),
            crate::models::StoredNodeLatency {
                definition_fingerprint: "fixture-fingerprint".into(),
                latency_ms: 142,
                measured_at_ms: 1_800_000_000_000,
            },
        );
        save_secrets(&paths, &secrets).unwrap();
        let disk = std::fs::read(&paths.secrets_file).unwrap();
        assert!(
            !disk
                .windows("private-path-token".len())
                .any(|window| window == b"private-path-token")
        );
        assert!(
            !disk
                .windows("fixture-private-password".len())
                .any(|window| window == b"fixture-private-password")
        );
        let loaded = load_secrets(&paths).unwrap();
        assert_eq!(
            loaded.subscriptions.get("svc").unwrap(),
            "https://fixture.invalid/sub/private-path-token"
        );
        let latency = loaded.node_latencies.get("node").unwrap();
        assert_eq!(latency.latency_ms, 142);
        assert_eq!(latency.measured_at_ms, 1_800_000_000_000);
    }
}
