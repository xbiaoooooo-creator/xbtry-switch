#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            config::status,
            config::fetch_models,
            config::import_config,
            config::restore_config
        ])
        .run(tauri::generate_context!())
        .expect("failed to start XBTry");
}
