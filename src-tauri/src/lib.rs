mod commands;
mod controller;
mod diagnostics;
mod history;
mod models;
mod network;
mod paths;
mod rules;
mod runtime;
mod security;
mod service_client;
mod single_instance;
mod store;
mod subscription;
mod tray;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if !single_instance::acquire() {
        return;
    }
    tauri::Builder::default()
        .setup(|app| {
            let runtime = runtime::AppRuntime::new(app.handle().clone())?;
            app.manage(runtime.clone());
            tray::setup(app.handle(), runtime.clone())?;
            runtime::setup_window_events(app.handle(), runtime.clone());
            tauri::async_runtime::spawn(async move {
                runtime.initialize().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::set_vpn,
            commands::set_selection,
            commands::preview_import,
            commands::cancel_import,
            commands::confirm_import,
            commands::refresh_service,
            commands::test_node,
            commands::test_all_nodes,
            commands::cancel_node_test_sweep,
            commands::rename_service,
            commands::save_match_group,
            commands::delete_match_group,
            commands::save_ruleset,
            commands::delete_ruleset,
            commands::save_preset,
            commands::delete_preset,
            commands::apply_preset,
            commands::set_history_paused,
            commands::clear_history,
            commands::get_history,
            commands::get_live,
            commands::get_live_rates,
            commands::get_diagnostics,
            commands::save_custom_group,
            commands::delete_custom_group,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Clashfork");
}
