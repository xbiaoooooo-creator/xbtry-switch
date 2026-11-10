#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod ai;
mod config;
mod doctor;
mod monitor;

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
            config::restore_config,
            config::list_history,
            config::apply_history,
            config::list_providers,
            config::save_provider,
            config::delete_provider,
            doctor::run_checks,
            doctor::apply_fix,
            doctor::list_fixes,
            doctor::undo_fix,
            doctor::diagnose,
            doctor::detect_proxy,
            doctor::claude_version_info,
            doctor::run_terminal,
            doctor::check_ip,
            doctor::find_claude,
            doctor::launch_claude,
            monitor::monitor_start,
            monitor::monitor_stop,
            monitor::monitor_status,
            monitor::monitor_interval,
            account::account_captcha,
            account::account_send_code,
            account::account_auth,
            account::account_me,
            account::account_logout,
            account::account_keys,
            ai::ai_check,
            ai::ai_zh
        ])
        .run(tauri::generate_context!())
        .expect("failed to start XBTry");
}
