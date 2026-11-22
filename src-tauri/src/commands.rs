use crate::{
    models::{
        AddNodesInput, AppState, CustomAppGroup, DiagnosticEntry, HistoryInput, HistoryReport,
        ImportPreview, LiveSnapshot, SaveMatchGroupInput, SaveRulesetInput, Selection, Snapshot,
    },
    runtime::AppRuntime,
};
use std::sync::Arc;
use tauri::State;

type Runtime<'a> = State<'a, Arc<AppRuntime>>;

fn command_error(error: anyhow::Error) -> String {
    error.to_string()
}

#[tauri::command]
pub fn get_snapshot(runtime: Runtime<'_>) -> Snapshot {
    runtime.snapshot()
}

#[tauri::command]
pub async fn set_vpn(runtime: Runtime<'_>, enabled: bool) -> Result<Snapshot, String> {
    runtime.set_vpn(enabled).await.map_err(command_error)
}

#[tauri::command]
pub async fn set_selection(runtime: Runtime<'_>, selection: Selection) -> Result<Snapshot, String> {
    runtime
        .apply_selection(
            selection.mode,
            selection.ruleset_id,
            selection.global_node_id,
        )
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn preview_import(runtime: Runtime<'_>, input: String) -> Result<ImportPreview, String> {
    runtime.preview_import(input).await.map_err(command_error)
}

#[tauri::command]
pub fn cancel_import(runtime: Runtime<'_>, preview_id: String) {
    runtime.cancel_import(&preview_id);
}

#[tauri::command]
pub async fn confirm_import(
    runtime: Runtime<'_>,
    preview_id: String,
    title: Option<String>,
) -> Result<Snapshot, String> {
    runtime
        .confirm_import(&preview_id, title)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn refresh_service(runtime: Runtime<'_>, service_id: String) -> Result<Snapshot, String> {
    runtime
        .refresh_service(&service_id)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn rename_service(
    runtime: Runtime<'_>,
    service_id: String,
    title: String,
) -> Result<Snapshot, String> {
    runtime
        .rename_service(&service_id, &title)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn save_match_group(
    runtime: Runtime<'_>,
    input: SaveMatchGroupInput,
) -> Result<Snapshot, String> {
    runtime.save_match_group(input).await.map_err(command_error)
}

#[tauri::command]
pub async fn delete_match_group(
    runtime: Runtime<'_>,
    group_id: String,
) -> Result<Snapshot, String> {
    runtime
        .delete_match_group(&group_id)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn save_ruleset(
    runtime: Runtime<'_>,
    input: SaveRulesetInput,
) -> Result<Snapshot, String> {
    runtime.save_ruleset(input).await.map_err(command_error)
}

#[tauri::command]
pub async fn delete_ruleset(runtime: Runtime<'_>, ruleset_id: String) -> Result<Snapshot, String> {
    runtime
        .delete_ruleset(&ruleset_id)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn save_preset(runtime: Runtime<'_>, name: String) -> Result<Snapshot, String> {
    runtime.save_preset(name).await.map_err(command_error)
}

#[tauri::command]
pub async fn delete_preset(runtime: Runtime<'_>, preset_id: String) -> Result<Snapshot, String> {
    runtime
        .delete_preset(&preset_id)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub async fn apply_preset(runtime: Runtime<'_>, preset_id: String) -> Result<Snapshot, String> {
    runtime
        .apply_preset(&preset_id)
        .await
        .map_err(command_error)
}

#[tauri::command]
pub fn set_history_paused(runtime: Runtime<'_>, paused: bool) -> Result<Snapshot, String> {
    runtime.set_history_paused(paused).map_err(command_error)
}

#[tauri::command]
pub fn clear_history(runtime: Runtime<'_>, confirm: bool) -> Result<Snapshot, String> {
    if !confirm {
        return Err("Confirmation is required to clear traffic history.".to_owned());
    }
    runtime.clear_history().map_err(command_error)
}

#[tauri::command]
pub fn get_history(runtime: Runtime<'_>, input: HistoryInput) -> Result<HistoryReport, String> {
    runtime.history_report(input).map_err(command_error)
}

#[tauri::command]
pub fn get_live(runtime: Runtime<'_>) -> LiveSnapshot {
    runtime.live_snapshot()
}

#[tauri::command]
pub fn get_diagnostics(runtime: Runtime<'_>) -> Vec<DiagnosticEntry> {
    runtime.diagnostics_snapshot()
}

#[tauri::command]
pub fn save_custom_group(runtime: Runtime<'_>, group: CustomAppGroup) -> Result<Snapshot, String> {
    runtime.save_custom_group(group).map_err(command_error)
}

#[tauri::command]
pub fn delete_custom_group(runtime: Runtime<'_>, group_id: String) -> Result<Snapshot, String> {
    runtime
        .delete_custom_group(&group_id)
        .map_err(command_error)
}

#[allow(dead_code)]
fn _state_type_check(_: &AppState, _: &AddNodesInput) {}
