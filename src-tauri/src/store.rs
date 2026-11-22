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
    if state.schema_version != 1 {
        anyhow::bail!("unsupported local state schema");
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
    let group_ids: std::collections::BTreeSet<_> = state
        .match_groups
        .iter()
        .map(|group| group.id.clone())
        .collect();
    for ruleset in &mut state.rulesets {
        ruleset
            .entries
            .retain(|entry| group_ids.contains(&entry.match_group_id));
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
        assert_eq!(loaded.schema_version, 1);
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
        assert_eq!(value["miscellaneous"], "block");
        assert_eq!(value["entries"], serde_json::json!([]));
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
    }
}
