#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod commands;

#[cfg(target_os = "windows")]
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|_app| {
            #[cfg(target_os = "windows")]
            if let Some(window) = _app.get_webview_window("main") {
                let _ = window.set_decorations(false);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::engine_rank,
            commands::engine_advise,
            commands::engine_classify_risk,
            commands::strategy_agent_chat,
            commands::get_device_signer_info,
            commands::sign_integrity_payload,
            commands::verify_integrity_payload,
            commands::export_case_pdf,
            commands::export_backup_json,
            commands::import_backup_json
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
