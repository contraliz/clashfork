use crate::runtime::AppRuntime;
use anyhow::Result;
use std::sync::Arc;
use tauri::{
    AppHandle, Emitter, Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

const TRAY_ID: &str = "clashfork-tray";

pub fn setup(app: &AppHandle, runtime: Arc<AppRuntime>) -> Result<()> {
    let menu = build_menu(app, &runtime)?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Clashfork")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref().to_owned();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let runtime = app.state::<Arc<AppRuntime>>().inner().clone();
                match id.as_str() {
                    "show" => show_window(&app),
                    "toggle_vpn" => {
                        let enabled = !runtime.snapshot().runtime.vpn_active;
                        if let Err(error) = runtime.set_vpn(enabled).await {
                            let _ = app.emit("backend-error", error.to_string());
                        }
                        show_window(&app);
                    }
                    "quit" => {
                        runtime.shutdown().await;
                        app.exit(0);
                    }
                    other if other.starts_with("preset:") => {
                        let preset_id = other.trim_start_matches("preset:");
                        if let Err(error) = runtime.apply_preset(preset_id).await {
                            let _ = app.emit("backend-error", error.to_string());
                        }
                        show_window(&app);
                    }
                    _ => {}
                }
            });
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

pub fn refresh(runtime: &Arc<AppRuntime>) -> Result<()> {
    let app = runtime.app.clone();
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    let menu = build_menu(&app, runtime)?;
    tray.set_menu(Some(menu))?;
    Ok(())
}

fn build_menu(app: &AppHandle, runtime: &Arc<AppRuntime>) -> Result<Menu<tauri::Wry>> {
    let snapshot = runtime.snapshot();
    let show = MenuItem::with_id(app, "show", "Open Clashfork", true, None::<&str>)?;
    let vpn_label = if snapshot.runtime.vpn_active {
        "Turn VPN off"
    } else {
        "Turn VPN on"
    };
    let toggle = MenuItem::with_id(app, "toggle_vpn", vpn_label, true, None::<&str>)?;
    let presets = Submenu::with_id(app, "presets", "Run configurations", true)?;
    if snapshot.state.presets.is_empty() {
        let empty = MenuItem::with_id(
            app,
            "preset-empty",
            "No saved configurations",
            false,
            None::<&str>,
        )?;
        presets.append(&empty)?;
    } else {
        for preset in &snapshot.state.presets {
            let item = MenuItem::with_id(
                app,
                format!("preset:{}", preset.id),
                &preset.name,
                true,
                None::<&str>,
            )?;
            presets.append(&item)?;
        }
    }
    let status = MenuItem::with_id(
        app,
        "status",
        snapshot
            .runtime
            .active_summary
            .clone()
            .unwrap_or_else(|| "Disconnected".to_owned()),
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Clashfork", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    Menu::with_items(
        app,
        &[
            &show, &status, &separator, &toggle, &presets, &separator, &quit,
        ],
    )
    .map_err(Into::into)
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
