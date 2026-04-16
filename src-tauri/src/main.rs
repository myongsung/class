#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod commands;

use std::fs;
#[cfg(target_os = "windows")]
use self_update::backends::github::Update;
#[cfg(target_os = "windows")]
use tauri::Manager;
use tauri::{command, AppHandle};

#[command]
fn check_and_update(app: AppHandle) -> Result<String, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        return Ok("윈도우에서만 자동 업데이트를 지원합니다.".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let app_version = app.package_info().version.to_string();

        let status = Update::configure()
            .repo_owner("myongsung")
            .repo_name("roosycozy")
            .bin_name("roosycozy.exe")
            .show_download_progress(true)
            .current_version(&app_version)
            .build()
            .map_err(|e| format!("업데이트 설정 오류: {}", e))?
            .update()
            .map_err(|e| format!("업데이트 실행 오류: {}", e))?;

        if status.updated() {
            Ok(format!("업데이트 완료: 버전 {}", status.version()))
        } else {
            Ok("최신 버전입니다.".to_string())
        }
    }
}

fn cleanup_old_versions() {
    if let Ok(current_exe) = std::env::current_exe() {
        let old_exe = current_exe.with_extension("exe.old");
        if old_exe.exists() {
            let _ = fs::remove_file(old_exe);
        }
    }
}

fn main() {
    cleanup_old_versions();

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
            commands::strategy_model_status,
      commands::start_strategy_model_download,
      commands::download_strategy_models,
            commands::get_device_signer_info,
            commands::sign_integrity_payload,
            commands::verify_integrity_payload,
            commands::export_case_pdf,
            commands::export_backup_json,
            commands::import_backup_json,
            check_and_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
